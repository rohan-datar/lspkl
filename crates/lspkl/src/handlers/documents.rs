//! Editor requests and source retrieval against a captured document epoch.

use super::{
    Result,
    diagnostics::{analyze_document, open_document_analysis},
};
use crate::{perf, state::Snapshot};
use fluent_uri::Uri;
use lsp_types::*;
use lspkl_db::FileStatus;
use lspkl_features::positions::PositionConverter;
use lspkl_types::InFile;
use lspkl_vfs::{Source, SourceLoader};
use std::{path::PathBuf, sync::Arc};
use tracing::{error, info};

pub fn hover(snapshot: &Snapshot, params: HoverParams) -> Result<Option<Hover>> {
    let doc = &params.text_document_position_params;
    let uri_str = doc.text_document.uri.as_str();
    info!(uri = uri_str, "hover");

    let Some((file, tree)) = open_document_analysis(snapshot, uri_str)? else {
        return Ok(None);
    };
    let positions = PositionConverter::new(tree, snapshot.position_encoding);
    let Some(offset) = positions.offset(doc.position) else {
        return Ok(None);
    };
    let _compute = perf::Compute::start("hover");
    Ok(lspkl_features::hover::hover(
        &snapshot.db,
        file,
        InFile::new(file, offset),
    ))
}
pub fn definition(
    snapshot: &Snapshot,
    params: GotoDefinitionParams,
) -> Result<Option<GotoDefinitionResponse>> {
    let doc = &params.text_document_position_params;
    let uri_str = doc.text_document.uri.as_str();
    info!(uri = uri_str, "gotoDefinition");

    let Some((file, tree)) = open_document_analysis(snapshot, uri_str)? else {
        return Ok(None);
    };
    let encoding = snapshot.position_encoding;
    let positions = PositionConverter::new(tree, encoding);
    let Some(offset) = positions.offset(doc.position) else {
        return Ok(None);
    };
    let _compute = perf::Compute::start("goto_definition");
    Ok(lspkl_features::goto_definition::goto_definition(
        &snapshot.db,
        file,
        InFile::new(file, offset),
        |file| {
            let uri = file.uri(&snapshot.db);
            match uri.scheme().as_str() {
                "pkl" | "https" | "jar" => {
                    lspkl_vfs::to_editor_uri(uri).ok()?.as_str().parse().ok()
                }
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

    let Some((file, tree)) = open_document_analysis(snapshot, uri_str)? else {
        return Ok(None);
    };
    let encoding = snapshot.position_encoding;
    let Some(offset) = PositionConverter::new(tree, encoding).offset(doc.position) else {
        return Ok(None);
    };
    let uri_completion = lspkl_db::with_module_resolver(&snapshot.db, |resolver| {
        lspkl_features::module_uri_completion::completion(
            &snapshot.db,
            file,
            offset,
            encoding,
            resolver,
        )
    });
    if uri_completion.is_some() {
        return Ok(uri_completion);
    }
    let _compute = perf::Compute::start("completion");
    let positions = PositionConverter::new(tree, encoding);
    Ok(lspkl_features::completion::completion(
        &snapshot.db,
        file,
        InFile::new(file, offset),
        |range| positions.range(range),
    ))
}
pub fn resolve(snapshot: &Snapshot, item: CompletionItem) -> Result<CompletionItem> {
    Ok(lspkl_features::module_uri_completion::resolve(
        &snapshot.db,
        item,
    ))
}
pub fn code_action(
    snapshot: &Snapshot,
    params: CodeActionParams,
) -> Result<Option<CodeActionResponse>> {
    let uri_str = params.text_document.uri.as_str();
    info!(uri = uri_str, "codeAction");

    let Some(super::diagnostics::DocumentDiagnostics {
        tree,
        diagnostics: diags,
    }) = analyze_document(snapshot, uri_str)?
    else {
        return Ok(None);
    };
    let _compute = perf::Compute::start("code_action");
    let positions = PositionConverter::new(tree, snapshot.position_encoding);
    let actions =
        lspkl_features::code_action::code_action(&params, &diags, |range| positions.range(range));
    Ok(Some(CodeActionResponse::from(actions)))
}
/// Extract owned formatter input before starting a subprocess.
pub struct FormatRequest {
    source: Arc<str>,
    settings: Arc<lspkl_db::settings::WorkspaceSettings>,
    uri: String,
}
pub fn prepare_formatting(
    snapshot: &Snapshot,
    params: DocumentFormattingParams,
) -> Result<Option<FormatRequest>> {
    let uri = params.text_document.uri.as_str();
    Ok(snapshot.document(uri).map(|file| FormatRequest {
        source: Arc::clone(file.text(&snapshot.db)),
        settings: Arc::clone(&snapshot.settings),
        uri: uri.to_owned(),
    }))
}
pub fn formatting(request: FormatRequest) -> Result<Option<Vec<TextEdit>>> {
    match lspkl_features::formatting::formatting(
        &request.source,
        request.settings.cli_path(),
        request
            .settings
            .formatter
            .grammar_version
            .as_ref()
            .map(lspkl_db::settings::GrammarVersion::as_str),
    ) {
        Ok(edits) => Ok(Some(edits)),
        Err(error) => {
            error!(uri = request.uri, ?error, "formatting failed");
            Ok(None)
        }
    }
}

pub enum FileContentsRequest {
    Cached(Arc<Source>),
    Load {
        uri: Uri<String>,
        loader: Arc<dyn SourceLoader + Send + Sync>,
        cache_dir: PathBuf,
    },
}
pub struct FileContentsResult {
    pub source: Arc<Source>,
    pub loaded: bool,
}
pub fn prepare_file_contents(
    snapshot: &Snapshot,
    document: TextDocumentIdentifier,
) -> Result<FileContentsRequest> {
    let uri = lspkl_vfs::parse_source_uri(document.uri.as_str())?;
    if let Some(file) = snapshot.db.known_file(&uri)
        && file.status(&snapshot.db) != FileStatus::Missing
    {
        return Ok(FileContentsRequest::Cached(Arc::new(Source {
            url: file.uri(&snapshot.db).clone(),
            text: Arc::clone(file.text(&snapshot.db)),
        })));
    }
    Ok(FileContentsRequest::Load {
        uri,
        loader: snapshot.db.source_loader_handle(),
        cache_dir: snapshot.settings.effective_cache_dir().to_owned(),
    })
}
/// Disk/network work deliberately runs without a Salsa handle.
pub fn file_contents(request: FileContentsRequest) -> Result<FileContentsResult> {
    match request {
        FileContentsRequest::Cached(source) => Ok(FileContentsResult {
            source,
            loaded: false,
        }),
        FileContentsRequest::Load {
            uri,
            loader,
            cache_dir,
        } => {
            let source = match loader.load(&uri, &cache_dir) {
                Ok(source) => source,
                Err(_) if uri.scheme().as_str() == "https" => {
                    lspkl_vfs::StandardSourceLoader::new().fetch(&uri, &cache_dir)?
                }
                Err(error) => return Err(error.into()),
            };
            Ok(FileContentsResult {
                source,
                loaded: true,
            })
        }
    }
}
