//! Editor requests and source retrieval against a captured document epoch.

use super::{
    HandlerError, Result,
    diagnostics::{analyze_document, open_document_analysis},
};
use crate::{perf, settings::GrammarVersion, state::Snapshot};
use fluent_uri::Uri;
use lsp_features::positions::PositionConverter;
use lsp_types::*;
use pkl_types::InFile;
use tracing::{error, info};

pub fn hover(snapshot: &Snapshot, params: HoverParams) -> Result<Option<Hover>> {
    let doc = &params.text_document_position_params;
    let uri_str = doc.text_document.uri.as_str();
    info!(uri = uri_str, "hover");

    let Some(analysis) = open_document_analysis(snapshot, uri_str)? else {
        return Ok(None);
    };
    let positions = PositionConverter::new(&analysis.module.tree, snapshot.position_encoding);
    let Some(offset) = positions.offset(doc.position) else {
        return Ok(None);
    };
    let _compute = perf::Compute::start("hover");
    let ctx = analysis.context(snapshot.base_module());
    Ok(lsp_features::hover::hover(
        &ctx,
        InFile::new(analysis.module.file_id, offset),
    ))
}
pub fn definition(
    snapshot: &Snapshot,
    params: GotoDefinitionParams,
) -> Result<Option<GotoDefinitionResponse>> {
    let doc = &params.text_document_position_params;
    let uri_str = doc.text_document.uri.as_str();
    info!(uri = uri_str, "gotoDefinition");

    let Some(analysis) = open_document_analysis(snapshot, uri_str)? else {
        return Ok(None);
    };
    let encoding = snapshot.position_encoding;
    let positions = PositionConverter::new(&analysis.module.tree, encoding);
    let Some(offset) = positions.offset(doc.position) else {
        return Ok(None);
    };
    let _compute = perf::Compute::start("goto_definition");
    let ctx = analysis.context(snapshot.base_module());
    Ok(lsp_features::goto_definition::goto_definition(
        &ctx,
        InFile::new(analysis.module.file_id, offset),
        |file_id| {
            let registry = snapshot.file_registry.lock();
            let uri = registry.uri_for(file_id)?;
            match uri.scheme().as_str() {
                "pkl" | "https" | "jar" => pkl_vfs::to_editor_uri(uri).ok()?.as_str().parse().ok(),
                _ => uri.as_str().parse().ok(),
            }
        },
        |tree, range| PositionConverter::new(tree, encoding).range(range),
    ))
}
pub fn completion(
    snapshot: &Snapshot,
    params: CompletionParams,
) -> Result<Option<CompletionResponse>> {
    let doc = &params.text_document_position;
    let uri_str = doc.text_document.uri.as_str();
    info!(uri = uri_str, "completion");

    let Some(analysis) = open_document_analysis(snapshot, uri_str)? else {
        return Ok(None);
    };
    let encoding = snapshot.position_encoding;
    let Some(offset) = PositionConverter::new(&analysis.module.tree, encoding).offset(doc.position)
    else {
        return Ok(None);
    };
    let uri_completion = crate::module_uri_completion::completion(snapshot, &analysis, offset);
    if uri_completion.is_some() {
        return Ok(uri_completion);
    }
    let _compute = perf::Compute::start("completion");
    let ctx = analysis.context(snapshot.base_module());
    let positions = PositionConverter::new(&analysis.module.tree, encoding);
    Ok(lsp_features::completion::completion(
        &ctx,
        InFile::new(analysis.module.file_id, offset),
        |range| positions.range(range),
    ))
}
pub fn resolve(snapshot: &Snapshot, item: CompletionItem) -> Result<CompletionItem> {
    Ok(crate::module_uri_completion::resolve(snapshot, item))
}
pub fn code_action(
    snapshot: &Snapshot,
    params: CodeActionParams,
) -> Result<Option<CodeActionResponse>> {
    let uri_str = params.text_document.uri.as_str();
    info!(uri = uri_str, "codeAction");

    // Recompute rather than cache: the VFS holds the current contents,
    // so analysis is deterministic and there is no state to invalidate.
    let Some((analysis, diags)) = analyze_document(snapshot, uri_str)? else {
        return Ok(None);
    };
    let _compute = perf::Compute::start("code_action");
    let positions = PositionConverter::new(&analysis.module.tree, snapshot.position_encoding);
    let actions =
        lsp_features::code_action::code_action(&params, &diags, |range| positions.range(range));
    Ok(Some(CodeActionResponse::from(actions)))
}
pub fn formatting(
    snapshot: &Snapshot,
    params: DocumentFormattingParams,
) -> Result<Option<Vec<TextEdit>>> {
    let uri = params.text_document.uri.as_str();
    let Some(file) = snapshot.document(uri) else {
        return Ok(None);
    };
    let source = file.contents();
    let settings = &snapshot.settings;
    match lsp_features::formatting::formatting(
        &source,
        settings.cli_path(),
        settings
            .formatter
            .grammar_version
            .as_ref()
            .map(GrammarVersion::as_str),
    ) {
        Ok(edits) => Ok(Some(edits)),
        Err(err) => {
            error!(uri, ?err, "formatting failed");
            Ok(None)
        }
    }
}

pub fn file_contents(snapshot: &Snapshot, document: TextDocumentIdentifier) -> Result<String> {
    if let Some(file) = snapshot.document(document.uri.as_str()) {
        return Ok(file.contents());
    }
    let uri = Uri::parse(document.uri.as_str())
        .map_err(|err| HandlerError::InvalidParams(err.to_string()))?
        .to_owned();
    Ok(snapshot.file_manager.get_or_load(&uri)?.contents())
}
