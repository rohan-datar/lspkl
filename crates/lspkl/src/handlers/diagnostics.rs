//! Shared analysis preparation and protocol diagnostics conversion.

use super::Result;
use crate::{
    perf,
    state::{AnalysisSnapshot, Snapshot},
};
use lsp_features::positions::PositionConverter;
use lsp_types::{Diagnostic, DiagnosticSeverity};

pub(super) fn open_document_analysis(
    snapshot: &Snapshot,
    uri: &str,
) -> Result<Option<AnalysisSnapshot>> {
    let _compute = perf::Compute::start("prepare");
    Ok(snapshot.open_document_analysis(uri)?)
}

pub(super) fn analyze_document(
    snapshot: &Snapshot,
    uri: &str,
) -> Result<Option<(AnalysisSnapshot, Vec<pkl_diagnostics::Diagnostic>)>> {
    let Some(analysis) = open_document_analysis(snapshot, uri)? else {
        return Ok(None);
    };
    let _compute = perf::Compute::start("diagnostics");
    let ctx = analysis.context(snapshot.base_module());
    let diagnostics = snapshot
        .diagnostics()
        .analyze(&ctx, analysis.module.file_id);
    Ok(Some((analysis, diagnostics)))
}
pub fn diagnostics(snapshot: &Snapshot, uri: &str) -> Result<Vec<Diagnostic>> {
    let Some((analysis, diagnostics)) = analyze_document(snapshot, uri)? else {
        return Ok(Vec::new());
    };
    let _compute = perf::Compute::start("diagnostics_conversion");
    let positions = PositionConverter::new(&analysis.module.tree, snapshot.position_encoding);
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
