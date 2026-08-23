//! End-to-end completion tests.
//!
//! Each test drives the full dispatch (`completion`): cursor lookup →
//! provider fan-out → collected `CompletionResponse`. Positions are
//! LSP-style (0-indexed).

mod harness;

use tower_lsp_server::lsp_types::{
    CompletionParams, CompletionResponse, Position, TextDocumentIdentifier,
};

use lsp_features::completion::completion;

fn params(line: u32, character: u32) -> CompletionParams {
    CompletionParams {
        text_document_position: tower_lsp_server::lsp_types::TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///test.pkl".parse().unwrap(),
            },
            position: Position { line, character },
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
        context: None,
    }
}

fn labels(response: Option<CompletionResponse>) -> Vec<String> {
    match response.expect("expected completion results") {
        CompletionResponse::Array(items) => items.into_iter().map(|i| i.label).collect(),
        CompletionResponse::List(list) => list.items.into_iter().map(|i| i.label).collect(),
    }
}

/// Typing a partial identifier at module level should suggest the sibling
/// properties in scope (plus keywords).
#[test]
fn module_level_unqualified_access_suggests_sibling_properties() {
    let base = harness::load_test_base();
    //   line 0: foo = 1
    //   line 1: bar = fo
    //                ^ cursor on the partial reference `fo`
    let src = "foo = 1\nbar = fo\n";
    harness::with_snippet(&base, src, |_tree, ctx| {
        let items = labels(completion(ctx, &params(1, 7)));

        assert!(
            items.iter().any(|l| l == "foo"),
            "sibling property `foo` should be suggested; got: {items:?}",
        );
        assert!(
            items.iter().any(|l| l == "trace"),
            "expression-level keywords like `trace` should be suggested",
        );
    });
}

/// Completing after a qualified receiver should suggest members of the
/// receiver's type, including inherited base-module members.
#[test]
fn qualified_access_suggests_receiver_members() {
    let base = harness::load_test_base();
    //   line 0: nums: List<Int> = List(1, 2)
    //   line 1: x = nums.isEmpty
    //                    ^ cursor on the member name
    let src = "nums: List<Int> = List(1, 2)\nx = nums.isEmpty\n";
    harness::with_snippet(&base, src, |_tree, ctx| {
        let items = labels(completion(ctx, &params(1, 10)));

        assert!(
            items.iter().any(|l| l == "length"),
            "List.length should be suggested; got: {items:?}",
        );
        assert!(
            items.iter().any(|l| l == "join"),
            "List.join should be suggested; got: {items:?}",
        );
        assert!(
            !items.iter().any(|l| l == "keys"),
            "Mapping-only member `keys` must not appear on a List",
        );
    });
}

/// A string literal in a position typed as a union of string literals should
/// offer each allowed value (enum-like completion).
#[test]
fn string_literal_offers_union_alternatives() {
    let base = harness::load_test_base();
    //   line 0: dir: "dev"|"prod" = ""
    //                             ^^ cursor between the quotes of the value
    let src = "dir: \"dev\"|\"prod\" = \"\"\n";
    harness::with_snippet(&base, src, |_tree, ctx| {
        let items = labels(completion(ctx, &params(0, 21)));

        assert!(
            items.iter().any(|l| l == "\"dev\""),
            "literal alternative \"dev\" should be offered; got: {items:?}",
        );
        assert!(
            items.iter().any(|l| l == "\"prod\""),
            "literal alternative \"prod\" should be offered; got: {items:?}",
        );
    });
}

/// A string literal in an `amends` clause currently produces no candidates:
/// enumerating amenable modules requires the deferred Phase 6 package
/// machinery (`TODO(completion-module-uri)`).
#[test]
fn string_literal_in_amends_clause_yields_no_candidates_yet() {
    let base = harness::load_test_base();
    let src = "amends \"/nonexistent.pkl\"\n";
    harness::with_snippet(&base, src, |_tree, ctx| {
        assert!(
            completion(ctx, &params(0, 9)).is_none(),
            "amends-position completion is deferred with ModuleUriCompletionProvider",
        );
    });
}

/// Cursor on whitespace past any token: no completions at all.
#[test]
fn whitespace_returns_none() {
    let base = harness::load_test_base();
    let src = "x = 1\n";
    harness::with_snippet(&base, src, |_tree, ctx| {
        // Column 10 is well past `x = 1`.
        assert!(completion(ctx, &params(0, 10)).is_none());
    });
}

/// Inside a class body whose superclass declares an abstract method,
/// completing should offer to implement it.
#[test]
fn implement_method_suggests_abstract_superclass_methods() {
    let base = harness::load_test_base();
    let src = concat!(
        "abstract class Animal {\n",    // 0
        "  function speak(): String\n", // 1
        "}\n",                          // 2
        "class Dog extends Animal {\n", // 3
        "  \n",                         // 4 <- cursor here
        "}\n",                          // 5
    );
    harness::with_snippet(&base, src, |_tree, ctx| {
        let items = labels(completion(ctx, &params(4, 1)));

        let speak = items
            .iter()
            .find(|l| l.contains("function speak"))
            .expect("abstract method `speak` should be offered; got: {items:?}");
        assert!(
            speak.starts_with("function speak(): String"),
            "label should render the full signature; got: {speak}",
        );
    });
}
