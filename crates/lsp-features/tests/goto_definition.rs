//! `goto_definition` tests.
//!
//! Locations use the caller-provided file identity to editor URI mapping.

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

        let result = goto_definition(ctx, position, |_| Some(doc_uri()))
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

/// Standard library targets use the same URI callback as user files.
#[test]
fn stdlib_target_uses_uri_callback() {
    let base = harness::load_test_base();
    let src = "x: String = \"\"\n";
    harness::with_snippet(&base, src, |_tree, ctx| {
        let position = Position {
            line: 0,
            character: 3,
        };

        let uri = Uri::from_str("lspkl://stdlib/base.pkl").unwrap();
        let result = goto_definition(ctx, position, |id| {
            (id == pkl_types::FileId::BASE).then(|| uri.clone())
        })
        .unwrap();
        let GotoDefinitionResponse::Scalar(location) = result else {
            panic!("single location");
        };
        assert_eq!(location.uri, uri);
        assert!(goto_definition(ctx, position, |_| None).is_none());
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

        assert!(goto_definition(ctx, position, |_| Some(doc_uri())).is_none());
    });
}
