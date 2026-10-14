//! End-to-end completion tests.
//!
//! Each test drives the full dispatch (`completion`): cursor lookup →
//! provider fan-out → collected `CompletionResponse`. Cursors are byte offsets.

mod harness;

use lsp_types::CompletionResponse;

use lspkl_features::completion::completion;

fn labels(response: Option<CompletionResponse>) -> Vec<String> {
    match response.expect("expected completion results") {
        CompletionResponse::Array(items) => items.into_iter().map(|i| i.label).collect(),
        CompletionResponse::List(list) => list.items.into_iter().map(|i| i.label).collect(),
    }
}

#[test]
fn qualified_type_completion_uses_imports_despite_expression_parameter_shadowing() {
    let base = harness::load_test_base();
    base.add(2, "class Widget {}\n");
    let source =
        "import \"2.pkl\" as dep\nfunction make(dep: String): dep.Widget = new dep.Widget {}\n";
    harness::with_snippet(&base, source, |tree, ctx| {
        let offset = source.find("dep.Widget").unwrap() + "dep.Wi".len();
        let response = completion(
            ctx.db,
            ctx.root,
            lspkl_types::InFile::new(
                harness::file(ctx, 1),
                lspkl_syntax::TextSize::try_from(offset).unwrap(),
            ),
            |range| harness::range(tree, range),
        );
        let Some(CompletionResponse::Array(items)) = response else {
            panic!("expected type completions");
        };
        assert!(
            items.iter().any(|item| item.label == "Widget"
                && item.kind == Some(lsp_types::CompletionItemKind::CLASS)),
            "{items:?}"
        );
    });
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
        let items = labels(completion(
            ctx.db,
            ctx.root,
            lspkl_types::InFile::new(
                harness::file(ctx, 1),
                harness::offset(ctx.tree(harness::file(ctx, 1)).unwrap(), 1, 7),
            ),
            |range| harness::range(ctx.tree(harness::file(ctx, 1)).unwrap(), range),
        ));

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
        let items = labels(completion(
            ctx.db,
            ctx.root,
            lspkl_types::InFile::new(
                harness::file(ctx, 1),
                harness::offset(ctx.tree(harness::file(ctx, 1)).unwrap(), 1, 10),
            ),
            |range| harness::range(ctx.tree(harness::file(ctx, 1)).unwrap(), range),
        ));

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
        let response = completion(
            ctx.db,
            ctx.root,
            lspkl_types::InFile::new(
                harness::file(ctx, 1),
                harness::offset(ctx.tree(harness::file(ctx, 1)).unwrap(), 0, 21),
            ),
            |range| harness::range(ctx.tree(harness::file(ctx, 1)).unwrap(), range),
        );
        let Some(CompletionResponse::Array(candidates)) = &response else {
            panic!("literal candidates")
        };
        for candidate in candidates {
            let Some(lsp_types::CompletionTextEdit::Edit(edit)) = &candidate.text_edit else {
                continue;
            };
            let converter = lspkl_features::positions::PositionConverter::new(
                ctx.tree(harness::file(ctx, 1)).unwrap(),
                Default::default(),
            );
            let bytes = converter.text_range(edit.range).unwrap();
            let mut updated = src.to_string();
            updated.replace_range(std::ops::Range::<usize>::from(bytes), &edit.new_text);
            assert!(
                updated.ends_with("\"\n"),
                "closing quote must remain: {updated}"
            );
        }
        let items = labels(response);

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

/// Semantic completion offers nothing inside an `amends` URI; module URIs are
/// completed by `module_uri_completion`, which the server consults first.
#[test]
fn string_literal_in_amends_clause_is_left_to_module_uri_completion() {
    let base = harness::load_test_base();
    let src = "amends \"/nonexistent.pkl\"\n";
    harness::with_snippet(&base, src, |_tree, ctx| {
        assert!(
            completion(
                ctx.db,
                ctx.root,
                lspkl_types::InFile::new(
                    harness::file(ctx, 1),
                    harness::offset(ctx.tree(harness::file(ctx, 1)).unwrap(), 0, 9)
                ),
                |range| harness::range(ctx.tree(harness::file(ctx, 1)).unwrap(), range)
            )
            .is_none(),
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
        assert!(
            completion(
                ctx.db,
                ctx.root,
                lspkl_types::InFile::new(
                    harness::file(ctx, 1),
                    harness::offset(ctx.tree(harness::file(ctx, 1)).unwrap(), 0, 10)
                ),
                |range| { harness::range(ctx.tree(harness::file(ctx, 1)).unwrap(), range) }
            )
            .is_none()
        );
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
        let items = labels(completion(
            ctx.db,
            ctx.root,
            lspkl_types::InFile::new(
                harness::file(ctx, 1),
                harness::offset(ctx.tree(harness::file(ctx, 1)).unwrap(), 4, 1),
            ),
            |range| harness::range(ctx.tree(harness::file(ctx, 1)).unwrap(), range),
        ));

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

#[test]
fn constrained_literal_alias_expands_every_member_with_alias_details_and_typed_edits() {
    let base = harness::load_test_base();
    let src = concat!(
        "typealias Mode = (\"dev\" | \"prod\" | \"test\")(true)\n",
        "mode: Mode = #\"partial\"#\n",
    );
    harness::with_snippet(&base, src, |tree, ctx| {
        let value_start = src.rfind("partial").unwrap();
        let response = completion(
            ctx.db,
            ctx.root,
            lspkl_types::InFile::new(
                harness::file(ctx, 1),
                lspkl_syntax::TextSize::try_from(value_start + 2).unwrap(),
            ),
            |range| harness::range(tree, range),
        );
        let Some(CompletionResponse::Array(items)) = response else {
            panic!("expected literal completions");
        };
        let converter = lspkl_features::positions::PositionConverter::new(tree, Default::default());
        for value in ["dev", "prod", "test"] {
            let item = items
                .iter()
                .find(|item| item.label == format!("\"{value}\""))
                .unwrap_or_else(|| panic!("missing {value}: {items:?}"));
            assert_eq!(
                item.label_details
                    .as_ref()
                    .and_then(|details| details.detail.as_deref()),
                Some("Mode")
            );
            let Some(lsp_types::CompletionTextEdit::Edit(edit)) = &item.text_edit else {
                panic!("literal completion must use a typed text edit");
            };
            let bytes = std::ops::Range::<usize>::from(converter.text_range(edit.range).unwrap());
            assert_eq!(bytes, value_start..value_start + "partial".len());
            assert_eq!(edit.new_text, value);
            let mut updated = src.to_owned();
            updated.replace_range(bytes, &edit.new_text);
            assert!(updated.ends_with(&format!("#\"{value}\"#\n")));
        }
    });
}

#[test]
fn constrained_function_union_offers_every_lambda_shape() {
    let base = harness::load_test_base();
    let src = "callback: (Function0<String> | Function1<Int, String> | Function2<String, Boolean, String>)(true) = missing\n";
    harness::with_snippet(&base, src, |tree, ctx| {
        let items = completion(
            ctx.db,
            ctx.root,
            lspkl_types::InFile::new(
                harness::file(ctx, 1),
                lspkl_syntax::TextSize::try_from(src.rfind("missing").unwrap() + 2).unwrap(),
            ),
            |range| harness::range(tree, range),
        )
        .expect("lambda completions");
        let items = match items {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => list.items,
        };
        for (label, insert) in [
            ("() -> …", "() -> ${1}"),
            ("(i) -> …", "(${1:i}) -> ${2}"),
            (
                "(string, boolean) -> …",
                "(${1:string}, ${2:boolean}) -> ${3}",
            ),
        ] {
            let item = items
                .iter()
                .find(|item| item.label == label)
                .unwrap_or_else(|| panic!("missing {label}: {items:?}"));
            assert_eq!(item.insert_text.as_deref(), Some(insert));
            assert_eq!(
                item.insert_text_format,
                Some(lsp_types::InsertTextFormat::SNIPPET)
            );
        }
    });
}

#[test]
fn parameter_completion_shadows_module_property() {
    let base = harness::load_test_base();
    let src = "value = 1\nfunction use(value: String): String = value\n";
    harness::with_snippet(&base, src, |tree, ctx| {
        let offset = lspkl_syntax::TextSize::from(src.rfind("value").unwrap() as u32);
        let response = completion(
            ctx.db,
            ctx.root,
            lspkl_types::InFile::new(harness::file(ctx, 1), offset),
            |range| harness::range(tree, range),
        )
        .expect("parameter completion");
        let items = match response {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => list.items,
        };
        let values: Vec<_> = items.iter().filter(|item| item.label == "value").collect();
        assert_eq!(values.len(), 1, "shadowed names should appear only once");
        assert_eq!(
            values[0].kind,
            Some(lsp_types::CompletionItemKind::VARIABLE)
        );
        assert_eq!(values[0].detail.as_deref(), Some("String"));
    });
}

#[test]
fn implement_method_includes_inherited_abstract_methods() {
    let base = harness::load_test_base();
    let src = "abstract class Animal { function speak(): String }
abstract class Mammal extends Animal {}
class Dog extends Mammal {
  
}
";
    harness::with_snippet(&base, src, |tree, ctx| {
        let items = labels(completion(
            ctx.db,
            ctx.root,
            lspkl_types::InFile::new(harness::file(ctx, 1), harness::offset(tree, 3, 1)),
            |range| harness::range(tree, range),
        ));
        assert!(
            items
                .iter()
                .any(|label| label.starts_with("function speak(): String")),
            "{items:?}"
        );
    });
}
