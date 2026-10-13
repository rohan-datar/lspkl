//! Diagnostics against tracked file and import queries.

use super::{HandlerError, Result};
use crate::{perf, state::Snapshot};
use lsp_features::positions::PositionConverter;
use lsp_types::{Diagnostic, DiagnosticSeverity};
use pkl_db::File;
use pkl_syntax::ast::SyntaxTree;

pub(super) fn open_document_analysis<'a>(
    snapshot: &'a Snapshot,
    uri: &str,
) -> Result<Option<(File, &'a SyntaxTree)>> {
    let Some(file) = snapshot.document(uri) else {
        return Ok(None);
    };
    let tree = pkl_db::parse(&snapshot.db, file)
        .as_ref()
        .map_err(|error| HandlerError::Domain(error.to_string()))?;
    Ok(Some((file, tree)))
}

pub(super) fn analyze_document<'a>(
    snapshot: &'a Snapshot,
    uri: &str,
) -> Result<Option<(&'a SyntaxTree, Vec<pkl_diagnostics::Diagnostic>)>> {
    let Some(document) = open_document_analysis(snapshot, uri)? else {
        return Ok(None);
    };
    let _compute = perf::Compute::start("diagnostics");
    let diagnostics = snapshot
        .diagnostics()
        .analyze(&snapshot.context(document.0), document.0);
    Ok(Some((document.1, diagnostics)))
}
pub fn diagnostics(snapshot: &Snapshot, uri: &str) -> Result<Vec<Diagnostic>> {
    let Some((tree, diagnostics)) = analyze_document(snapshot, uri)? else {
        return Ok(Vec::new());
    };
    let _compute = perf::Compute::start("diagnostics_conversion");
    let positions = PositionConverter::new(tree, snapshot.position_encoding);
    Ok(diagnostics
        .iter()
        .map(|diag| to_lsp_diagnostic(diag, &positions))
        .collect())
}
fn to_lsp_diagnostic(
    diag: &pkl_diagnostics::Diagnostic,
    positions: &PositionConverter<'_>,
) -> Diagnostic {
    Diagnostic {
        range: positions.range(diag.span),
        severity: Some(match diag.severity {
            pkl_diagnostics::Severity::Error => DiagnosticSeverity::ERROR,
            pkl_diagnostics::Severity::Warning => DiagnosticSeverity::WARNING,
            pkl_diagnostics::Severity::Info => DiagnosticSeverity::INFORMATION,
            pkl_diagnostics::Severity::Hint => DiagnosticSeverity::HINT,
        }),
        source: Some("lspkl".to_string()),
        message: diag.message.clone(),
        ..Diagnostic::default()
    }
}
