//! `LanguageServer` trait implementation.

use std::sync::Arc;
use std::sync::atomic::Ordering;

#[path = "perf.rs"]
mod perf;

use tower_lsp_server::lsp_types::*;
use tower_lsp_server::{Client, LanguageServer, jsonrpc};

use fluent_uri::Uri;
use tracing::{error, info, warn};

use lsp_features::positions::{PositionConverter, PositionEncoding};
use pkl_project::dto::PackageUri;
use pkl_types::AnalysisContext;

use crate::settings::{GrammarVersion, WorkspaceSettings};
use crate::state::{AnalysisSnapshot, ServerState};

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

    /// Prepare request-pinned trees for an already-open document and its imports.
    ///
    /// Returns `None` for an unknown or malformed root URI. The root is not
    /// loaded from disk; reachable imports may be. Syntax errors still produce
    /// an error-tolerant tree, while preparation failures are logged.
    ///
    async fn open_document_analysis(&self, uri_str: &str) -> Option<AnalysisSnapshot> {
        let url = Uri::parse(uri_str.to_owned()).ok()?;
        let file = self.state.file_manager.get(&url)?;
        let state = Arc::clone(&self.state);
        match perf::blocking("prepare", move || {
            state
                .parse_module(file)
                .and_then(|module| state.prepare_analysis(module))
        })
        .await
        {
            Ok(Ok(analysis)) => Some(analysis),
            Ok(Err(err)) => {
                error!(uri = uri_str, ?err, "analysis preparation failed");
                None
            }
            Err(err) => {
                error!(uri = uri_str, ?err, "analysis worker failed");
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
    async fn analyze_document(
        &self,
        uri_str: &str,
    ) -> Option<(AnalysisSnapshot, Vec<pkl_diagnostics::Diagnostic>)> {
        let analysis = self.open_document_analysis(uri_str).await?;
        let _compute = perf::Compute::start("diagnostics");
        let ctx = AnalysisContext::new(
            &analysis.module.tree,
            analysis.module.file_id,
            self.state.base_module(),
            &analysis.trees,
            &analysis.imports,
        );
        let diagnostics = self.state.diagnostics().analyze(&ctx);
        Some((analysis, diagnostics))
    }

    /// Push current diagnostics for a just-opened or just-changed document
    /// to the editor (`textDocument/publishDiagnostics`).
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip(self), fields(handler_id = tracing::field::Empty))]
    async fn publish(&self, uri_str: &str, version: Option<i32>) {
        let _perf = perf::Handler::enter();
        let Some(client) = &self.client else {
            return;
        };
        let Ok(uri) = uri_str.parse::<tower_lsp_server::lsp_types::Uri>() else {
            error!(uri = uri_str, "publish: failed to parse URI");
            return;
        };
        let analyzed = self.analyze_document(uri_str).await;
        let compute = perf::Compute::start("diagnostics_conversion");
        let lsp_diags = analyzed.map_or_else(Vec::new, |(analysis, diags)| {
            let encoding = *self.state.position_encoding.read();
            let positions = PositionConverter::new(&analysis.module.tree, encoding);
            diags
                .iter()
                .map(|diag| to_lsp_diagnostic(diag, &positions))
                .collect()
        });
        drop(compute);
        client.publish_diagnostics(uri, lsp_diags, version).await;
    }
}

/// Convert an internal diagnostic to its LSP wire form.
fn to_lsp_diagnostic(
    diag: &pkl_diagnostics::Diagnostic,
    positions: &PositionConverter<'_>,
) -> Diagnostic {
    Diagnostic {
        range: positions.range(diag.span),
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
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "initialize"))]
    async fn initialize(&self, params: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        let _perf = perf::Handler::enter();
        info!(?params.client_info, "initialize");
        let encoding = if params
            .capabilities
            .general
            .as_ref()
            .and_then(|general| general.position_encodings.as_ref())
            .is_some_and(|encodings| encodings.contains(&PositionEncodingKind::UTF8))
        {
            PositionEncoding::Utf8
        } else {
            PositionEncoding::Utf16
        };
        *self.state.position_encoding.write() = encoding;
        self.state.client_configuration.store(
            params
                .capabilities
                .workspace
                .as_ref()
                .and_then(|caps| caps.configuration)
                .unwrap_or(false),
            Ordering::Relaxed,
        );
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

        self.restore_workspaces(folders).await;
        if let Some(options) = params.initialization_options {
            match WorkspaceSettings::parse(options) {
                Ok(settings) => self.state.apply_settings(settings),
                Err(err) => error!(?err, "invalid initialization settings"),
            }
        }

        Ok(InitializeResult {
            server_info: None,
            capabilities: ServerCapabilities {
                position_encoding: Some(match encoding {
                    PositionEncoding::Utf8 => PositionEncodingKind::UTF8,
                    PositionEncoding::Utf16 => PositionEncodingKind::UTF16,
                }),
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
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["pkl.downloadPackage".into(), "pkl.syncProjects".into()],
                    work_done_progress_options: Default::default(),
                }),
                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(OneOf::Left(true)),
                    }),
                    file_operations: None,
                }),
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

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "initialized"))]
    async fn initialized(&self, _params: InitializedParams) {
        let _perf = perf::Handler::enter();
        info!("server initialized");
        self.refresh_configuration().await;
    }

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "did_change_configuration"))]
    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        let _perf = perf::Handler::enter();
        if params.settings.is_null()
            || params
                .settings
                .as_object()
                .is_some_and(|settings| settings.is_empty())
        {
            self.refresh_configuration().await;
        } else {
            match WorkspaceSettings::parse(params.settings) {
                Ok(settings) => self.state.apply_settings(settings),
                Err(err) => error!(?err, "invalid workspace settings"),
            }
        }
        self.publish_open_documents().await;
    }

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "did_change_workspace_folders"))]
    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        let _perf = perf::Handler::enter();
        let mut folders = self
            .state
            .project_manager
            .lock()
            .workspace_folders()
            .to_vec();
        for folder in params.event.removed {
            if let Ok(uri) = Uri::parse(folder.uri.as_str().to_owned())
                && let Ok(path) = pkl_vfs::url_to_path(&uri)
            {
                folders.retain(|existing| existing != &path);
            }
        }
        for folder in params.event.added {
            if let Ok(uri) = Uri::parse(folder.uri.as_str().to_owned())
                && let Ok(path) = pkl_vfs::url_to_path(&uri)
                && !folders.contains(&path)
            {
                folders.push(path);
            }
        }
        self.restore_workspaces(folders).await;
        let settings = {
            let current = Arc::clone(&self.state.settings.read());
            current.rebind(self.state.project_manager.lock().workspace_folders())
        };
        self.state.apply_settings(settings);
        self.refresh_configuration().await;
        self.publish_open_documents().await;
    }

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "shutdown"))]
    async fn shutdown(&self) -> jsonrpc::Result<()> {
        let _perf = perf::Handler::enter();
        info!("server shutdown");
        Ok(())
    }

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "execute_command"))]
    async fn execute_command(
        &self,
        mut params: ExecuteCommandParams,
    ) -> jsonrpc::Result<Option<serde_json::Value>> {
        let _perf = perf::Handler::enter();
        match params.command.as_str() {
            "pkl.downloadPackage" => {
                if params.arguments.len() != 1 {
                    return Err(jsonrpc::Error::invalid_params(
                        "pkl.downloadPackage expects one package URI string",
                    ));
                }
                let Some(serde_json::Value::String(uri)) = params.arguments.pop() else {
                    return Err(jsonrpc::Error::invalid_params(
                        "pkl.downloadPackage expects one package URI string",
                    ));
                };
                self.download_package(uri).await?;
            }
            "pkl.syncProjects" => {
                if !params.arguments.is_empty() {
                    return Err(jsonrpc::Error::invalid_params(
                        "pkl.syncProjects expects no arguments",
                    ));
                }
                self.sync_projects().await?;
            }
            _ => return Err(jsonrpc::Error::invalid_params("unknown Pkl command")),
        }
        Ok(None)
    }

    // ---- Text document lifecycle ----

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "did_open"))]
    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let _perf = perf::Handler::enter();
        let uri = params.text_document.uri;
        let uri_str = uri.as_str();
        info!(uri = %uri_str, "didOpen");
        let Ok(url) = Uri::parse(uri_str.to_string()) else {
            error!(uri = %uri_str, "Failed to parse URI!");
            return;
        };

        let version = params.text_document.version;
        let file = self
            .state
            .file_manager
            .open(url, params.text_document.text, Some(version));
        self.state
            .open_documents
            .lock()
            .insert(self.state.file_id(&file), Arc::new(uri));
        self.publish_open_documents().await;
    }

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "did_change"))]
    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let _perf = perf::Handler::enter();
        let uri = params.text_document.uri;
        let uri_str = uri.as_str();
        info!(uri = %uri_str, "didChange");
        let Ok(url) = Uri::parse(uri_str.to_string()) else {
            error!(uri = %uri_str, "Failed to parse URI!");
            return;
        };

        let version = params.text_document.version;
        let mut changes = params.content_changes;
        let Some(change) = changes.pop() else {
            return;
        };
        if let Some(stored_version) = self
            .state
            .file_manager
            .get(&url)
            .and_then(|file| file.version())
            && version <= stored_version
        {
            let ordering_violations = perf::ordering_violation();
            warn!(target: "lspkl::perf", uri = uri_str, incoming_version = version,
                stored_version, ordering_violations, "non-increasing didChange version; applying incoming text");
        }
        match self
            .state
            .file_manager
            .update(&url, change.text, Some(version))
        {
            Ok(_) => {
                self.publish_open_documents().await;
            }
            Err(err) => {
                error!(uri = %uri_str, url = %url.clone(), err = ?err, "update failed!")
            }
        }
    }

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "did_save"))]
    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let _perf = perf::Handler::enter();
        let _ = params;
        // No-op. Later phases may trigger project/package refresh here
    }

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "did_close"))]
    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let _perf = perf::Handler::enter();
        info!(uri = %params.text_document.uri.as_str(), "didClose");
        let Ok(url) = Uri::parse(params.text_document.uri.as_str().to_string()) else {
            error!(uri = %params.text_document.uri.as_str(), "Failed to parse URI!");
            return;
        };

        if let Some(file) = self.state.file_manager.get(&url) {
            self.state
                .open_documents
                .lock()
                .remove(&self.state.file_id(&file));
        }
        self.state.file_manager.close(&url);
        self.publish_open_documents().await;
    }

    // ---- LSP features ----

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "hover"))]
    async fn hover(&self, params: HoverParams) -> jsonrpc::Result<Option<Hover>> {
        let _perf = perf::Handler::enter();
        let doc = &params.text_document_position_params;
        let uri_str = doc.text_document.uri.as_str();
        info!(uri = uri_str, "hover");

        let Some(analysis) = self.open_document_analysis(uri_str).await else {
            return Ok(None);
        };
        let positions =
            PositionConverter::new(&analysis.module.tree, *self.state.position_encoding.read());
        let Some(offset) = positions.offset(doc.position) else {
            return Ok(None);
        };
        let _compute = perf::Compute::start("hover");
        let ctx = AnalysisContext::new(
            &analysis.module.tree,
            analysis.module.file_id,
            self.state.base_module(),
            &analysis.trees,
            &analysis.imports,
        );
        Ok(lsp_features::hover::hover(&ctx, offset))
    }

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "goto_definition"))]
    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> jsonrpc::Result<Option<GotoDefinitionResponse>> {
        let _perf = perf::Handler::enter();
        let doc = &params.text_document_position_params;
        let uri_str = doc.text_document.uri.as_str();
        info!(uri = uri_str, "gotoDefinition");

        let Some(analysis) = self.open_document_analysis(uri_str).await else {
            return Ok(None);
        };
        let encoding = *self.state.position_encoding.read();
        let positions = PositionConverter::new(&analysis.module.tree, encoding);
        let Some(offset) = positions.offset(doc.position) else {
            return Ok(None);
        };
        let _compute = perf::Compute::start("goto_definition");
        let ctx = AnalysisContext::new(
            &analysis.module.tree,
            analysis.module.file_id,
            self.state.base_module(),
            &analysis.trees,
            &analysis.imports,
        );
        Ok(lsp_features::goto_definition::goto_definition(
            &ctx,
            offset,
            |file_id| {
                if file_id == pkl_types::FileId::BASE {
                    return pkl_vfs::to_editor_uri(&Uri::parse("pkl:base".to_owned()).ok()?)
                        .ok()?
                        .as_str()
                        .parse()
                        .ok();
                }
                let registry = self.state.file_registry.lock();
                let uri = registry.uri_for(file_id)?;
                match uri.scheme().as_str() {
                    "pkl" | "https" | "jar" => {
                        pkl_vfs::to_editor_uri(uri).ok()?.as_str().parse().ok()
                    }
                    _ => uri.as_str().parse().ok(),
                }
            },
            |tree, range| PositionConverter::new(tree, encoding).range(range),
        ))
    }

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "completion"))]
    async fn completion(
        &self,
        params: CompletionParams,
    ) -> jsonrpc::Result<Option<CompletionResponse>> {
        let _perf = perf::Handler::enter();
        let doc = &params.text_document_position;
        let uri_str = doc.text_document.uri.as_str();
        info!(uri = uri_str, "completion");

        let Some(analysis) = self.open_document_analysis(uri_str).await else {
            return Ok(None);
        };
        let encoding = *self.state.position_encoding.read();
        let Some(offset) =
            PositionConverter::new(&analysis.module.tree, encoding).offset(doc.position)
        else {
            return Ok(None);
        };
        let state = Arc::clone(&self.state);
        let (analysis, uri_completion) = perf::blocking("module_uri_completion", move || {
            let result = crate::module_uri_completion::completion(&state, &analysis, offset);
            (analysis, result)
        })
        .await
        .map_err(|err| jsonrpc::Error {
            code: jsonrpc::ErrorCode::InternalError,
            message: err.to_string().into(),
            data: None,
        })?;
        if uri_completion.is_some() {
            return Ok(uri_completion);
        }
        let _compute = perf::Compute::start("completion");
        let ctx = AnalysisContext::new(
            &analysis.module.tree,
            analysis.module.file_id,
            self.state.base_module(),
            &analysis.trees,
            &analysis.imports,
        );
        let positions = PositionConverter::new(&analysis.module.tree, encoding);
        Ok(lsp_features::completion::completion(
            &ctx,
            offset,
            |range| positions.range(range),
        ))
    }

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "completion_resolve"))]
    async fn completion_resolve(&self, item: CompletionItem) -> jsonrpc::Result<CompletionItem> {
        let _perf = perf::Handler::enter();
        let state = Arc::clone(&self.state);
        perf::blocking("completion_resolve", move || {
            crate::module_uri_completion::resolve(&state, item)
        })
        .await
        .map_err(|err| jsonrpc::Error {
            code: jsonrpc::ErrorCode::InternalError,
            message: err.to_string().into(),
            data: None,
        })
    }

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "code_action"))]
    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> jsonrpc::Result<Option<CodeActionResponse>> {
        let _perf = perf::Handler::enter();
        let uri_str = params.text_document.uri.as_str();
        info!(uri = uri_str, "codeAction");

        // Recompute rather than cache: the VFS holds the current contents,
        // so analysis is deterministic and there is no state to invalidate.
        let Some((analysis, diags)) = self.analyze_document(uri_str).await else {
            return Ok(None);
        };
        let _compute = perf::Compute::start("code_action");
        let positions =
            PositionConverter::new(&analysis.module.tree, *self.state.position_encoding.read());
        let actions =
            lsp_features::code_action::code_action(&params, &diags, |range| positions.range(range));
        Ok(Some(CodeActionResponse::from(actions)))
    }

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "formatting"))]
    async fn formatting(
        &self,
        params: DocumentFormattingParams,
    ) -> jsonrpc::Result<Option<Vec<TextEdit>>> {
        let _perf = perf::Handler::enter();
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

        let settings = Arc::clone(&self.state.settings.read());
        let format = perf::blocking("formatting", move || {
            lsp_features::formatting::formatting(
                &source,
                settings.cli_path(),
                settings
                    .formatter
                    .grammar_version
                    .as_ref()
                    .map(GrammarVersion::as_str),
            )
        })
        .await;
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

    async fn restore_workspaces(&self, folders: Vec<std::path::PathBuf>) {
        let state = Arc::clone(&self.state);
        let result = perf::blocking("restore_workspaces", move || {
            let mut manager = state.project_manager.lock();
            manager.initialize(folders);
            let roots = manager.workspace_folders().to_vec();
            for root in roots {
                let path = root.join(".lspkl/projects.json");
                match path.try_exists() {
                    Ok(true) => {
                        if let Err(err) = manager.load_state(&path) {
                            error!(path = %path.display(), ?err, "could not restore project state");
                        }
                    }
                    Ok(false) => {}
                    Err(err) => {
                        error!(path = %path.display(), ?err, "could not inspect project state")
                    }
                }
            }
        })
        .await;
        if let Err(err) = result {
            error!(?err, "project state restoration worker failed");
        }
    }

    async fn publish_open_documents(&self) {
        let documents: Vec<_> = self.state.open_documents.lock().values().cloned().collect();
        for uri in documents {
            let version = Uri::parse(uri.as_str()).ok().and_then(|uri| {
                self.state
                    .file_manager
                    .get(&uri.to_owned())
                    .and_then(|file| file.version())
            });
            self.publish(uri.as_str(), version).await;
        }
    }

    async fn refresh_configuration(&self) {
        if !self.state.client_configuration.load(Ordering::Relaxed) {
            return;
        }
        let Some(client) = &self.client else {
            return;
        };
        let sections = [
            "pkl.cli.path",
            "pkl.formatter.grammarVersion",
            "pkl.modulepath",
            "pkl.projects.excludedDirectories",
            "pkl.cacheDir",
        ];
        let params = ConfigurationParams {
            items: sections
                .iter()
                .map(|section| ConfigurationItem {
                    scope_uri: None,
                    section: Some((*section).to_owned()),
                })
                .collect(),
        };
        match client.configuration(params.items).await {
            Ok(values) if values.len() == sections.len() => {
                let [cli, grammar, modulepath, excluded, cache]: [serde_json::Value; 5] =
                    values.try_into().unwrap();
                let value = serde_json::json!({
                    "cli": {"path": cli}, "formatter": {"grammarVersion": grammar},
                    "modulepath": if modulepath.is_null() { serde_json::json!([]) } else { modulepath },
                    "projects": {"excludedDirectories": if excluded.is_null() { serde_json::json!([]) } else { excluded }},
                    "cacheDir": cache,
                });
                match WorkspaceSettings::parse(value) {
                    Ok(settings) => self.state.apply_settings(settings),
                    Err(err) => error!(?err, "invalid client configuration"),
                }
            }
            Ok(_) => error!("client returned an incomplete configuration response"),
            Err(err) => error!(?err, "workspace configuration request failed"),
        }
    }

    /// Read source behind the same synthetic URIs returned by definition requests.
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "file_contents"))]
    pub async fn file_contents(&self, document: TextDocumentIdentifier) -> jsonrpc::Result<String> {
        let _perf = perf::Handler::enter();
        let uri = Uri::parse(document.uri.as_str())
            .map(|uri| uri.to_owned())
            .map_err(|err| jsonrpc::Error {
                code: jsonrpc::ErrorCode::InvalidParams,
                message: err.to_string().into(),
                data: None,
            })?;
        let state = Arc::clone(&self.state);
        perf::blocking("file_contents", move || {
            state
                .file_manager
                .get_or_load(&uri)
                .map(|file| file.contents())
        })
        .await
        .map_err(|err| jsonrpc::Error {
            code: jsonrpc::ErrorCode::InternalError,
            message: err.to_string().into(),
            data: None,
        })?
        .map_err(|err| jsonrpc::Error {
            code: jsonrpc::ErrorCode::InternalError,
            message: err.to_string().into(),
            data: None,
        })
    }

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "sync_projects"))]
    pub async fn sync_projects(&self) -> jsonrpc::Result<()> {
        let _perf = perf::Handler::enter();
        let state = Arc::clone(&self.state);
        let sync = perf::blocking("sync_projects", move || {
            state.project_manager.lock().sync_projects()
        })
        .await;
        match sync {
            Ok(Ok(_)) => {
                self.publish_open_documents().await;
                Ok(())
            }
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
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "download_package"))]
    pub async fn download_package(&self, uri: String) -> jsonrpc::Result<()> {
        let _perf = perf::Handler::enter();
        let package_uri = PackageUri::create(&uri).map_err(|err| jsonrpc::Error {
            code: jsonrpc::ErrorCode::InvalidParams,
            message: err.to_string().into(),
            data: None,
        })?;
        let cache_dir = self
            .state
            .settings
            .read()
            .resolved_cache_dir
            .clone()
            .or_else(|| self.state.package_cache_dir.clone())
            .ok_or_else(|| jsonrpc::Error {
                code: jsonrpc::ErrorCode::InternalError,
                message: "cannot locate home directory for the Pkl package cache".into(),
                data: None,
            })?;
        let state = Arc::clone(&self.state);
        let download = perf::blocking("download_package", move || {
            state
                .package_manager
                .lock()
                .download_package(&package_uri, &cache_dir)
        })
        .await;
        match download {
            Ok(Ok(())) => {
                self.publish_open_documents().await;
                Ok(())
            }
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
