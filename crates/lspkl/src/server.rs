//! `LanguageServer` trait implementation.

use std::sync::Arc;

use tokio::task::spawn_blocking;

use tower_lsp_server::lsp_types::*;
use tower_lsp_server::{Client, LanguageServer, jsonrpc};

use fluent_uri::Uri;
use tracing::{error, info};

use pkl_project::dto::PackageUri;
use pkl_syntax::ast::SyntaxTree;
use pkl_types::{AnalysisContext, FileId};

use crate::state::ServerState;

/// Placeholder id for the document a request is made against.
const DOCUMENT_FILE_ID: FileId = FileId(1);

#[derive(Debug)]
pub struct PklBackend {
    pub state: Arc<ServerState>,
    /// `None` in tests, `Some` when wired up by `LspService::new`.
    pub client: Option<Client>,
}

impl PklBackend {
    pub fn new(state: Arc<ServerState>, client: Option<Client>) -> Self {
        Self { state, client }
    }

    /// Helper for tests: a backend with no client wired up.
    pub fn for_testing() -> Self {
        Self {
            state: Arc::new(ServerState::new()),
            client: None,
        }
    }

    /// Look up `uri_str` in the file manager and parse its current contents.
    ///
    /// Returns `None` when the editor never sent a `didOpen` for the file or
    /// the URI is malformed; parse *errors* still return a tree (tree-sitter
    /// is error-tolerant), so `None` from the parser itself is unexpected
    /// and logged.
    ///
    /// TODO(caching): this re-parses on every request.
    ///
    /// TODO(blocking): parsing is sync CPU work on the async worker. Fine at
    /// typical file sizes; wrap in `spawn_blocking` if it ever shows up in
    /// request latency.
    fn open_document_tree(&self, uri_str: &str) -> Option<SyntaxTree> {
        let Ok(url) = Uri::parse(uri_str.to_string()) else {
            error!(uri = uri_str, "failed to parse URI");
            return None;
        };
        let file = self.state.file_manager.get(&url)?;
        match pkl_syntax::parse(&file.contents()) {
            Ok(tree) => Some(tree),
            Err(err) => {
                error!(uri = uri_str, ?err, "parse failed");
                None
            }
        }
    }

    /// Run the full analyzer pipeline against the current contents of
    /// `uri_str`.
    ///
    /// Returns `None` when the file isn't open or can't be parsed; an
    /// unparseable-but-open file still yields whatever diagnostics the
    /// analyzers produce on the error-tolerant tree.
    fn analyze_document(&self, uri_str: &str) -> Option<Vec<pkl_diagnostics::Diagnostic>> {
        let tree = self.open_document_tree(uri_str)?;
        let ctx = AnalysisContext::new(&tree, DOCUMENT_FILE_ID, self.state.base_module());
        Some(self.state.diagnostics().analyze(&ctx))
    }

    /// Push current diagnostics for a just-opened or just-changed document
    /// to the editor (`textDocument/publishDiagnostics`).
    async fn publish(&self, uri_str: &str, version: Option<i32>) {
        let Some(client) = &self.client else {
            return;
        };
        let Ok(uri) = uri_str.parse::<tower_lsp_server::lsp_types::Uri>() else {
            error!(uri = uri_str, "publish: failed to parse URI");
            return;
        };
        let diags = self.analyze_document(uri_str).unwrap_or_default();
        let lsp_diags = diags.iter().map(to_lsp_diagnostic).collect();
        client.publish_diagnostics(uri, lsp_diags, version).await;
    }
}

/// Convert an internal diagnostic to its LSP wire form.
fn to_lsp_diagnostic(diag: &pkl_diagnostics::Diagnostic) -> Diagnostic {
    Diagnostic {
        range: lsp_features::span_to_range(&diag.span),
        severity: Some(match diag.severity {
            pkl_diagnostics::Severity::Error => {
                tower_lsp_server::lsp_types::DiagnosticSeverity::ERROR
            }
            pkl_diagnostics::Severity::Warning => {
                tower_lsp_server::lsp_types::DiagnosticSeverity::WARNING
            }
            pkl_diagnostics::Severity::Info => {
                tower_lsp_server::lsp_types::DiagnosticSeverity::INFORMATION
            }
            pkl_diagnostics::Severity::Hint => {
                tower_lsp_server::lsp_types::DiagnosticSeverity::HINT
            }
        }),
        source: Some("lspkl".to_string()),
        message: diag.message.clone(),
        ..Diagnostic::default()
    }
}

impl LanguageServer for PklBackend {
    async fn initialize(&self, params: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        info!(?params.client_info, "initialize");
        let uris: Vec<_> = match params.workspace_folders {
            Some(folders) => folders.into_iter().map(|folder| folder.uri).collect(),
            None => {
                #[allow(deprecated)]
                let root_uri = params.root_uri;
                root_uri.into_iter().collect()
            }
        };

        let folders: Vec<_> = uris
            .into_iter()
            .filter_map(|uri| {
                let parsed = Uri::parse(uri.as_str().to_owned())
                    .map_err(|err| {
                        error!(?err, "unable to parse workspace root uri");
                    })
                    .ok()?;
                pkl_vfs::file_manager::url_to_path(&parsed)
                    .map_err(|err| {
                        error!(?err, "unable to convert {parsed} to file path");
                    })
                    .ok()
            })
            .collect();

        self.state.project_manager.lock().initialize(folders);

        Ok(InitializeResult {
            server_info: None,
            capabilities: ServerCapabilities {
                position_encoding: None,
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                notebook_document_sync: None,
                selection_range_provider: None,
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(true),
                    trigger_characters: Some(
                        [".", "/", "\"", ":"]
                            .into_iter()
                            .map(str::to_string)
                            .chain(('a'..='z').map(|c| c.to_string()))
                            .chain(('A'..='Z').map(|c| c.to_string()))
                            .collect(),
                    ),
                    all_commit_characters: None,
                    work_done_progress_options: WorkDoneProgressOptions {
                        work_done_progress: None,
                    },
                    completion_item: None,
                }),
                signature_help_provider: None,
                definition_provider: Some(OneOf::Left(true)),
                type_definition_provider: None,
                implementation_provider: None,
                references_provider: None,
                document_highlight_provider: None,
                document_symbol_provider: None,
                workspace_symbol_provider: None,
                code_action_provider: Some(CodeActionProviderCapability::Options(
                    CodeActionOptions {
                        code_action_kinds: Some(vec![CodeActionKind::QUICKFIX]),
                        work_done_progress_options: WorkDoneProgressOptions {
                            work_done_progress: None,
                        },
                        resolve_provider: None,
                    },
                )),
                code_lens_provider: None,
                document_formatting_provider: Some(OneOf::Left(true)),
                document_range_formatting_provider: None,
                document_on_type_formatting_provider: None,
                rename_provider: None,
                document_link_provider: None,
                color_provider: None,
                folding_range_provider: None,
                declaration_provider: None,
                execute_command_provider: None,
                workspace: None,
                call_hierarchy_provider: None,
                semantic_tokens_provider: None,
                moniker_provider: None,
                linked_editing_range_provider: None,
                inline_value_provider: None,
                inlay_hint_provider: None,
                diagnostic_provider: None,
                experimental: None,
            },
        })
    }

    async fn initialized(&self, params: InitializedParams) {
        let _ = params;
        info!("server intialized")
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        info!("server shutdown");
        Ok(())
    }

    // ---- Text document lifecycle ----

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let uri_str = uri.as_str();
        info!(uri = %uri_str, "didOpen");
        let Ok(url) = Uri::parse(uri_str.to_string()) else {
            error!(uri = %uri_str, "Failed to parse URI!");
            return;
        };

        let version = params.text_document.version;
        self.state
            .file_manager
            .open(url.clone(), params.text_document.text, Some(version));
        self.publish(uri_str, Some(version)).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let uri_str = uri.as_str();
        info!(uri = %uri_str, "didChange");
        let Ok(url) = Uri::parse(uri_str.to_string()) else {
            error!(uri = %uri_str, "Failed to parse URI!");
            return;
        };

        let version = params.text_document.version;
        match self.state.file_manager.update(
            &url,
            params.content_changes[0].text.clone(),
            Some(version),
        ) {
            Ok(_) => {
                // Re-publish on every change so stale diagnostics clear.
                self.publish(uri_str, Some(version)).await;
            }
            Err(err) => {
                error!(uri = %uri_str, url = %url.clone(), err = ?err, "update failed!")
            }
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let _ = params;
        // No-op. Later phases may trigger project/package refresh here
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        info!(uri = %params.text_document.uri.as_str(), "didClose");
        let Ok(url) = Uri::parse(params.text_document.uri.as_str().to_string()) else {
            error!(uri = %params.text_document.uri.as_str(), "Failed to parse URI!");
            return;
        };

        self.state.file_manager.close(&url);
    }

    // ---- LSP features ----

    async fn hover(&self, params: HoverParams) -> jsonrpc::Result<Option<Hover>> {
        let doc = &params.text_document_position_params;
        let uri_str = doc.text_document.uri.as_str();
        info!(uri = uri_str, "hover");

        let Some(tree) = self.open_document_tree(uri_str) else {
            return Ok(None);
        };
        let ctx = AnalysisContext::new(&tree, DOCUMENT_FILE_ID, self.state.base_module());
        Ok(lsp_features::hover::hover(&ctx, doc.position))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> jsonrpc::Result<Option<GotoDefinitionResponse>> {
        let doc = &params.text_document_position_params;
        let uri_str = doc.text_document.uri.as_str();
        info!(uri = uri_str, "gotoDefinition");

        let Some(tree) = self.open_document_tree(uri_str) else {
            return Ok(None);
        };
        let ctx = AnalysisContext::new(&tree, DOCUMENT_FILE_ID, self.state.base_module());
        Ok(lsp_features::goto_definition::goto_definition(
            &ctx,
            doc.position,
            &doc.text_document.uri,
        ))
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> jsonrpc::Result<Option<CompletionResponse>> {
        let doc = &params.text_document_position;
        let uri_str = doc.text_document.uri.as_str();
        info!(uri = uri_str, "completion");

        let Some(tree) = self.open_document_tree(uri_str) else {
            return Ok(None);
        };
        let ctx = AnalysisContext::new(&tree, DOCUMENT_FILE_ID, self.state.base_module());
        Ok(lsp_features::completion::completion(&ctx, &params))
    }

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> jsonrpc::Result<Option<CodeActionResponse>> {
        let uri_str = params.text_document.uri.as_str();
        info!(uri = uri_str, "codeAction");

        // Recompute rather than cache: the VFS holds the current contents,
        // so analysis is deterministic and there is no state to invalidate.
        let Some(diags) = self.analyze_document(uri_str) else {
            return Ok(None);
        };
        let actions = lsp_features::code_action::code_action(&params, &diags);
        Ok(Some(CodeActionResponse::from(actions)))
    }

    async fn formatting(
        &self,
        params: DocumentFormattingParams,
    ) -> jsonrpc::Result<Option<Vec<TextEdit>>> {
        let uri_str = params.text_document.uri.as_str();
        info!(uri = uri_str, "formatting");

        let Ok(url) = Uri::parse(uri_str.to_string()) else {
            error!(uri = uri_str, "formatting: failed to parse URI");
            return Ok(None);
        };

        let Some(file) = self.state.file_manager.get(&url) else {
            // Editor asked us to format a file we never saw a `didOpen` for.
            // Return None rather than an error — nothing to do.
            return Ok(None);
        };
        let source = file.contents();

        let format = spawn_blocking(move || lsp_features::formatting::formatting(&source)).await;
        match format {
            Ok(Ok(edits)) => Ok(Some(edits)),
            Ok(Err(err)) => {
                error!(uri = uri_str, ?err, "formatting failed");
                Ok(None)
            }
            Err(err) => {
                error!(uri = uri_str, ?err, "formatter panicked");
                Ok(None)
            }
        }
    }
}

impl PklBackend {
    // ---- Custom `pkl/*` RPC methods ----

    pub async fn sync_projects(&self) -> jsonrpc::Result<()> {
        let state = Arc::clone(&self.state);
        let sync = spawn_blocking(move || state.project_manager.lock().sync_projects()).await;
        match sync {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(err)) => {
                error!(?err, "syncProjects failed");
                Err(jsonrpc::Error {
                    code: jsonrpc::ErrorCode::InternalError,
                    message: err.to_string().into(),
                    data: None,
                })
            }
            Err(err) => {
                error!(?err, "syncProjects panicked");
                Err(jsonrpc::Error {
                    code: jsonrpc::ErrorCode::InternalError,
                    message: err.to_string().into(),
                    data: None,
                })
            }
        }
    }

    /// Client asks us to download a package by URI string.
    pub async fn download_package(&self, uri: String) -> jsonrpc::Result<()> {
        let package_uri = PackageUri::create(&uri).map_err(|err| jsonrpc::Error {
            code: jsonrpc::ErrorCode::InvalidParams,
            message: err.to_string().into(),
            data: None,
        })?;
        let cache_dir = self
            .state
            .package_cache_dir
            .clone()
            .ok_or_else(|| jsonrpc::Error {
                code: jsonrpc::ErrorCode::InternalError,
                message: "cannot locate home directory for the Pkl package cache".into(),
                data: None,
            })?;
        let state = Arc::clone(&self.state);
        let download = spawn_blocking(move || {
            state
                .package_manager
                .lock()
                .download_package(&package_uri, &cache_dir)
        })
        .await;
        match download {
            Ok(Ok(())) => Ok(()),
            Ok(Err(err)) => {
                error!(?err, "downloadPackage failed");
                Err(jsonrpc::Error {
                    code: jsonrpc::ErrorCode::InternalError,
                    message: err.to_string().into(),
                    data: None,
                })
            }
            Err(err) => {
                error!(?err, "downloadPackage panicked");
                Err(jsonrpc::Error {
                    code: jsonrpc::ErrorCode::InternalError,
                    message: err.to_string().into(),
                    data: None,
                })
            }
        }
    }
}
