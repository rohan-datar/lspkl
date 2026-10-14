//! Diagnostics against tracked file and import queries.

use super::{HandlerError, Result};
use crate::{perf, state::Snapshot};
use lsp_types::Diagnostic;
use lspkl_db::File;
use lspkl_features::positions::PositionConverter;
use lspkl_syntax::ast::SyntaxTree;

pub(super) struct DocumentDiagnostics<'a> {
    pub tree: &'a SyntaxTree,
    pub diagnostics: std::sync::Arc<[lspkl_diagnostics::Diagnostic]>,
}

pub(super) fn open_document_analysis<'a>(
    snapshot: &'a Snapshot,
    uri: &str,
) -> Result<Option<(File, &'a SyntaxTree)>> {
    let Some(file) = snapshot.document(uri) else {
        return Ok(None);
    };
    let tree = lspkl_db::parse(&snapshot.db, file)
        .as_ref()
        .map_err(|error| HandlerError::Domain(error.to_string()))?;
    Ok(Some((file, tree)))
}

pub(super) fn analyze_document<'a>(
    snapshot: &'a Snapshot,
    uri: &str,
) -> Result<Option<DocumentDiagnostics<'a>>> {
    let Some(document) = open_document_analysis(snapshot, uri)? else {
        return Ok(None);
    };
    let _compute = perf::Compute::start("diagnostics");
    let diagnostics = lspkl_diagnostics::diagnostics(&snapshot.db, document.0);
    Ok(Some(DocumentDiagnostics {
        tree: document.1,
        diagnostics,
    }))
}
pub fn diagnostics(snapshot: &Snapshot, uri: &str) -> Result<Vec<Diagnostic>> {
    let Some(DocumentDiagnostics { tree, diagnostics }) = analyze_document(snapshot, uri)? else {
        return Ok(Vec::new());
    };
    let _compute = perf::Compute::start("diagnostics_conversion");
    let positions = PositionConverter::new(tree, snapshot.position_encoding);
    Ok(diagnostics
        .iter()
        .map(|diag| lspkl_features::diagnostics::to_lsp(diag, positions.range(diag.span)))
        .collect())
}
