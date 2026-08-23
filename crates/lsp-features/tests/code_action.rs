//! `code_action` tests.
mod harness;

use std::str::FromStr;

use pkl_diagnostics::{Diagnostic, QuickFix, Severity, TextEdit};
use tower_lsp_server::lsp_types::{
    CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams, Position,
    TextDocumentIdentifier, Uri,
};

use lsp_features::code_action::code_action;
use lsp_features::span_to_range;

fn doc_uri() -> Uri {
    Uri::from_str("file:///snippets/test.pkl").expect("static test URI must parse")
}

/// Mirror of what the server will do when it publishes diagnostics: convert
/// an internal `Diagnostic` into its LSP wire form. Kept local until the
/// publish path lands (`TODO(publish-diagnostics)`); if it grows, promote it
/// into a shared crate rather than duplicating per-handler.
fn to_lsp(d: &Diagnostic) -> tower_lsp_server::lsp_types::Diagnostic {
    tower_lsp_server::lsp_types::Diagnostic {
        range: span_to_range(&d.span),
        severity: match d.severity {
            Severity::Error => Some(tower_lsp_server::lsp_types::DiagnosticSeverity::ERROR),
            Severity::Warning => Some(tower_lsp_server::lsp_types::DiagnosticSeverity::WARNING),
            Severity::Info => Some(tower_lsp_server::lsp_types::DiagnosticSeverity::INFORMATION),
            Severity::Hint => Some(tower_lsp_server::lsp_types::DiagnosticSeverity::HINT),
        },
        message: d.message.clone(),
        ..Default::default()
    }
}

fn params_for(echoed: &[tower_lsp_server::lsp_types::Diagnostic]) -> CodeActionParams {
    CodeActionParams {
        text_document: TextDocumentIdentifier { uri: doc_uri() },
        range: Default::default(),
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
        context: CodeActionContext {
            diagnostics: echoed.to_vec(),
            only: None,
            trigger_kind: None,
        },
    }
}

/// A diagnostic with one quick fix that inserts `-> Unknown` after the
/// property's type annotation:
///
/// ```text
///   fix: annotate as unknown
///     insert "-> Unknown" at 2:4
/// ```
fn annotated_diagnostic() -> Diagnostic {
    let mut d = Diagnostic::error(
        pkl_syntax::ast::Span {
            begin_line: 2,
            begin_col: 3,
            end_line: 2,
            end_col: 9,
            begin_byte: 14,
            end_byte: 20,
        },
        "Type mismatch. Expected: Int. Actual: String",
    );
    d.quick_fixes.push(QuickFix::single(
        "annotate as unknown",
        TextEdit::insert_after(
            pkl_syntax::ast::Span {
                begin_line: 2,
                begin_col: 9,
                end_line: 2,
                end_col: 9,
                begin_byte: 20,
                end_byte: 20,
            },
            " -> Unknown",
        ),
    ));
    d
}

/// A diagnostic with no fixes must contribute nothing to the response
#[test]
fn fixless_diagnostic_yields_no_actions() {
    let fixless = Diagnostic::warning(
        pkl_syntax::ast::Span {
            begin_line: 5,
            begin_col: 1,
            end_line: 5,
            end_col: 4,
            begin_byte: 40,
            end_byte: 43,
        },
        "unresolved reference: foo",
    );
    // A record with fixes sits alongside in our set, but only the fixless
    // diagnostic is echoed here — it must not materialize any action.
    let internal = vec![annotated_diagnostic(), fixless];
    let echoed = [to_lsp(&internal[1])];
    let params = params_for(&echoed);

    let result = code_action(&params, &internal);
    assert!(
        result.is_empty(),
        "diagnostic without quick_fixes should produce no actions, got {result:?}"
    );
}

#[allow(clippy::mutable_key_type)]
#[test]
fn matching_diagnostic_materializes_its_quick_fixes() {
    let internal = vec![annotated_diagnostic()];
    let echoed = internal.iter().map(to_lsp).collect::<Vec<_>>();
    let params = params_for(&echoed);

    let result = code_action(&params, &internal);
    assert_eq!(result.len(), 1, "one quick fix -> one action");

    let CodeActionOrCommand::CodeAction(action) = &result[0] else {
        panic!("expected a CodeAction, got {:?}", result[0]);
    };
    assert_eq!(action.title, "annotate as unknown");
    assert_eq!(
        action.kind.as_ref().map(CodeActionKind::as_str),
        Some(CodeActionKind::QUICKFIX.as_str()),
        "quick fixes must carry kind = quickfix"
    );
    assert_eq!(action.is_preferred, Some(true));

    let edit = action.edit.as_ref().expect("edit action carries edits");
    // Inspect through the public shape; single-file changes map under the
    // request URI. (Field access kept structural so this doubles as a spec.)
    let changes = edit
        .changes
        .as_ref()
        .expect("local edits use WorkspaceEdit.changes");
    let (_, edits) = changes
        .iter()
        .find(|(uri, _)| **uri == doc_uri())
        .expect("edits keyed by the request URI");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].new_text, " -> Unknown");
    // insert-after at 2:9 (1-based) -> zero-based start = end = 1:8
    assert_eq!(
        edits[0].range.start,
        Position {
            line: 1,
            character: 8
        }
    );
    assert_eq!(
        edits[0].range.end,
        Position {
            line: 1,
            character: 8
        }
    );
}

/// A client-echoed diagnostic that matches nothing in our records (e.g. it
/// came from a stale publish) is ignored, not errored.
#[test]
fn unmatched_echoed_diagnostics_are_skipped() {
    let internal = vec![annotated_diagnostic()];
    let mut stale = to_lsp(&annotated_diagnostic());
    stale.message = "stale message from a previous publish".into();
    let params = params_for(&[stale]);

    let result = code_action(&params, &internal);
    assert!(
        result.is_empty(),
        "no internal diagnostic matches the echoed one; got {result:?}"
    );
}
