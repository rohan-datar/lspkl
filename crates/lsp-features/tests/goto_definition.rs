//! `goto_definition` tests.
//!
//! Same-file targets are the only ones that can produce a `Location` today:
//! there is no `FileId → URI` mapping until Phase 6's `FileManager`, so
//! cross-file targets (in practice: pkl:base) return `None` — see
//! `TODO(stdlib-goto)` in the implementation.

mod harness;

use std::str::FromStr;

use tower_lsp_server::lsp_types::{GotoDefinitionResponse, Position, Uri};

use lsp_features::goto_definition::goto_definition;

fn doc_uri() -> Uri {
    Uri::from_str("file:///snippets/test.pkl").expect("static test URI must parse")
}

/// Snippet (positions are 0-indexed because LSP):
///   line 0: x = 1
///   line 1: y = x
///                ^ cursor on the `x` reference (line=1, character=4)
///
/// Goto-definition should land on the `x = 1` declaration, in the same
/// document the request was made against.
#[test]
fn same_file_definition_returns_location_in_document() {
    let base = harness::load_test_base();
    let src = "x = 1\ny = x\n";
    harness::with_snippet(&base, src, |_tree, ctx| {
        let position = Position {
            line: 1,
            character: 4,
        };

        let result = goto_definition(ctx, position, &doc_uri())
            .expect("`x` in `y = x` should resolve to a definition");

        let GotoDefinitionResponse::Scalar(location) = result else {
            panic!("expected a single Location, got {result:?}");
        };
        assert_eq!(
            location.uri,
            doc_uri(),
            "same-file target reuses the request URI"
        );
        assert_eq!(
            location.range.start.line, 0,
            "declaration `x = 1` is on line 0",
        );
    });
}

/// `String` resolves into pkl:base. Until `TODO(stdlib-goto)` lands we
/// return `None` rather than a URI the editor can't open.
#[test]
fn stdlib_target_is_deferred_and_returns_none() {
    let base = harness::load_test_base();
    let src = "x: String = \"\"\n";
    harness::with_snippet(&base, src, |_tree, ctx| {
        let position = Position {
            line: 0,
            character: 3,
        };

        assert!(
            goto_definition(ctx, position, &doc_uri()).is_none(),
            "stdlib targets are deferred until a FileId → URI mapping exists",
        );
    });
}

/// Cursor on whitespace past the end of the content: nothing to resolve.
#[test]
fn whitespace_returns_none() {
    let base = harness::load_test_base();
    let src = "x = 1\n";
    harness::with_snippet(&base, src, |_tree, ctx| {
        let position = Position {
            line: 0,
            character: 10,
        };

        assert!(goto_definition(ctx, position, &doc_uri()).is_none());
    });
}
