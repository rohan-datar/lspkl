//! Thin asynchronous transport adapter over synchronous snapshot handlers.
use crate::{
    handlers::{self, HandlerError},
    perf,
    settings::WorkspaceSettings,
    state::{Snapshot, Workspace},
};
use lsp_types::*;
use parking_lot::Mutex;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::watch;
use tower_lsp_server::{Client, LanguageServer, jsonrpc};
use tracing::{error, info};

impl From<tokio::task::JoinError> for HandlerError {
    fn from(error: tokio::task::JoinError) -> Self {
        Self::Domain(error.to_string())
    }
}

impl From<HandlerError> for jsonrpc::Error {
    fn from(error: HandlerError) -> Self {
        jsonrpc::Error {
            code: if matches!(error, HandlerError::InvalidParams(_)) {
                jsonrpc::ErrorCode::InvalidParams
            } else {
                jsonrpc::ErrorCode::InternalError
            },
            message: error.to_string().into(),
            data: None,
        }
    }
}

#[derive(Debug)]
pub struct PklBackend {
    pub workspace: Arc<Mutex<Workspace>>,
    pub client: Option<Client>,
    diagnostic_workers: Mutex<HashMap<Uri, watch::Sender<Option<Snapshot>>>>,
}
impl PklBackend {
    pub fn new(workspace: Arc<Mutex<Workspace>>, client: Option<Client>) -> Self {
        Self {
            workspace,
            client,
            diagnostic_workers: Mutex::new(HashMap::new()),
        }
    }
    pub fn for_testing() -> Self {
        Self::new(Arc::new(Mutex::new(Workspace::new())), None)
    }

    async fn dispatch<R: Send + 'static>(
        &self,
        stage: &'static str,
        handler: impl FnOnce(&Snapshot) -> handlers::Result<R> + Send + 'static,
    ) -> jsonrpc::Result<R> {
        let _perf = perf::Handler::enter();
        let snapshot = self.workspace.lock().snapshot();
        perf::blocking(stage, move || handler(&snapshot))
            .await
            .map_err(HandlerError::from)?
            .map_err(Into::into)
    }

    /// Queue one latest snapshot per document. A busy worker never builds a FIFO
    /// of edits: it drains the watch slot after compute before starting again.
    fn schedule_diagnostics(&self) {
        let Some(client) = &self.client else {
            return;
        };
        let workspace_guard = self.workspace.lock();
        let snapshot = workspace_guard.snapshot();
        let mut workers = self.diagnostic_workers.lock();
        workers.retain(|uri, sender| {
            if snapshot.document(uri.as_str()).is_none() {
                sender.send_replace(None);
                false
            } else {
                true
            }
        });
        for uri in snapshot.open_documents.values() {
            if let Some(sender) = workers.get(uri.as_ref()) {
                sender.send_replace(Some(snapshot.clone()));
                continue;
            }
            let (sender, mut receiver) = watch::channel(Some(snapshot.clone()));
            workers.insert(uri.as_ref().clone(), sender);
            let workspace = Arc::clone(&self.workspace);
            let client = client.clone();
            let uri = uri.as_ref().clone();
            tokio::spawn(async move {
                loop {
                    let current = receiver.borrow_and_update().clone();
                    let Some(current) = current else {
                        break;
                    };
                    let generation = current.generation;
                    let version = current
                        .document(uri.as_str())
                        .and_then(|file| file.version());
                    let analysis_uri = uri.clone();
                    let result = perf::blocking("diagnostics", move || {
                        handlers::diagnostics::diagnostics(&current, analysis_uri.as_str())
                    })
                    .await;
                    // Includes dependency/settings edits whose root document version
                    // did not change, and close/reopen of the same URI.
                    match receiver.has_changed() {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(_) => break,
                    }
                    let valid = {
                        let workspace = workspace.lock();
                        let latest = workspace.snapshot();
                        latest.generation == generation && latest.document(uri.as_str()).is_some()
                    };
                    if valid {
                        match result {
                            Ok(Ok(diagnostics)) => {
                                tokio::select! {
                                    biased;
                                    changed = receiver.changed() => { if changed.is_err() { break; } continue; }
                                    _ = client.publish_diagnostics(uri.clone(), diagnostics, version) => {}
                                }
                            }
                            Ok(Err(err)) => error!(?err, "diagnostic analysis failed"),
                            Err(err) => error!(?err, "diagnostic worker failed"),
                        }
                    }
                    if receiver.changed().await.is_err() {
                        break;
                    }
                }
            });
        }
    }
    fn refresh_analysis(&self) {
        self.workspace.lock().invalidate_analysis();
        self.schedule_diagnostics();
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "refresh_configuration"))]
    async fn refresh_configuration(&self) {
        if !self.workspace.lock().snapshot().client_configuration {
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
        let items = sections
            .iter()
            .map(|section| ConfigurationItem {
                scope_uri: None,
                section: Some((*section).to_owned()),
            })
            .collect();
        match client.configuration(items).await {
            Ok(values) if values.len() == sections.len() => {
                let [cli, grammar, modulepath, excluded, cache]: [serde_json::Value; 5] =
                    values.try_into().unwrap();
                let value = serde_json::json!({
                    "cli": {"path": cli}, "formatter": {"grammarVersion": grammar},
                    "modulepath": if modulepath.is_null() { serde_json::json!([]) } else { modulepath },
                    "projects": {"excludedDirectories": if excluded.is_null() { serde_json::json!([]) } else { excluded }}, "cacheDir": cache,
                });
                match WorkspaceSettings::parse(value) {
                    Ok(settings) => {
                        self.workspace.lock().apply_settings(settings);
                        self.schedule_diagnostics();
                    }
                    Err(err) => error!(?err, "invalid client configuration"),
                }
            }
            Ok(_) => error!("client returned an incomplete configuration response"),
            Err(err) => error!(?err, "workspace configuration request failed"),
        }
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "file_contents"))]
    pub async fn file_contents(&self, document: TextDocumentIdentifier) -> jsonrpc::Result<String> {
        self.dispatch("file_contents", move |snapshot| {
            handlers::documents::file_contents(snapshot, document)
        })
        .await
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "sync_projects"))]
    pub async fn sync_projects(&self) -> jsonrpc::Result<()> {
        self.dispatch("sync_projects", handlers::commands::sync_projects)
            .await?;
        self.refresh_analysis();
        Ok(())
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "download_package"))]
    pub async fn download_package(&self, uri: String) -> jsonrpc::Result<()> {
        self.dispatch("download_package", move |snapshot| {
            handlers::commands::download_package(snapshot, uri)
        })
        .await?;
        self.refresh_analysis();
        Ok(())
    }
}
impl LanguageServer for PklBackend {
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "initialize"))]
    async fn initialize(&self, params: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        let _perf = perf::Handler::enter();
        let result = handlers::lifecycle::initialize(&mut self.workspace.lock(), &params);
        self.dispatch(
            "restore_workspaces",
            handlers::lifecycle::restore_workspaces,
        )
        .await?;
        Ok(result)
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "initialized"))]
    async fn initialized(&self, _: InitializedParams) {
        let _perf = perf::Handler::enter();
        info!("server initialized");
        self.refresh_configuration().await;
    }

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "shutdown"))]
    async fn shutdown(&self) -> jsonrpc::Result<()> {
        let _perf = perf::Handler::enter();
        info!("server shutdown");
        Ok(())
    }

    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "did_open"))]
    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let _perf = perf::Handler::enter();
        if let Err(err) = handlers::lifecycle::did_open(&mut self.workspace.lock(), params) {
            error!(?err, "didOpen failed");
            return;
        }
        self.schedule_diagnostics();
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "did_change"))]
    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let _perf = perf::Handler::enter();
        if let Err(err) = handlers::lifecycle::did_change(&mut self.workspace.lock(), params) {
            error!(?err, "didChange failed");
            return;
        }
        self.schedule_diagnostics();
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "did_close"))]
    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let _perf = perf::Handler::enter();
        if let Err(err) = handlers::lifecycle::did_close(&mut self.workspace.lock(), params) {
            error!(?err, "didClose failed");
            return;
        }
        self.schedule_diagnostics();
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "did_save"))]
    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let _perf = perf::Handler::enter();
        if let Err(err) = handlers::lifecycle::did_save(&mut self.workspace.lock(), params) {
            error!(?err, "didSave failed");
        }
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
            if let Err(err) =
                handlers::lifecycle::did_change_configuration(&mut self.workspace.lock(), params)
            {
                error!(?err, "invalid configuration settings");
                return;
            }
            self.schedule_diagnostics();
        }
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "did_change_workspace_folders"))]
    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        let _perf = perf::Handler::enter();
        handlers::lifecycle::did_change_workspace_folders(&mut self.workspace.lock(), params);
        if let Err(err) = self
            .dispatch(
                "restore_workspaces",
                handlers::lifecycle::restore_workspaces,
            )
            .await
        {
            error!(?err, "project state restoration failed");
        }
        self.refresh_configuration().await;
        self.schedule_diagnostics();
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "execute_command"))]
    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> jsonrpc::Result<Option<serde_json::Value>> {
        let result = self
            .dispatch("execute_command", move |snapshot| {
                handlers::commands::execute_command(snapshot, params)
            })
            .await?;
        self.refresh_analysis();
        Ok(result)
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "hover"))]
    async fn hover(&self, params: HoverParams) -> jsonrpc::Result<Option<Hover>> {
        self.dispatch("hover", move |snapshot| {
            handlers::documents::hover(snapshot, params)
        })
        .await
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "goto_definition"))]
    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> jsonrpc::Result<Option<GotoDefinitionResponse>> {
        self.dispatch("goto_definition", move |snapshot| {
            handlers::documents::definition(snapshot, params)
        })
        .await
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "completion"))]
    async fn completion(
        &self,
        params: CompletionParams,
    ) -> jsonrpc::Result<Option<CompletionResponse>> {
        self.dispatch("completion", move |snapshot| {
            handlers::documents::completion(snapshot, params)
        })
        .await
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "completion_resolve"))]
    async fn completion_resolve(&self, item: CompletionItem) -> jsonrpc::Result<CompletionItem> {
        self.dispatch("completion_resolve", move |snapshot| {
            handlers::documents::resolve(snapshot, item)
        })
        .await
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "code_action"))]
    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> jsonrpc::Result<Option<CodeActionResponse>> {
        self.dispatch("code_action", move |snapshot| {
            handlers::documents::code_action(snapshot, params)
        })
        .await
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "formatting"))]
    async fn formatting(
        &self,
        params: DocumentFormattingParams,
    ) -> jsonrpc::Result<Option<Vec<TextEdit>>> {
        // Formatter errors, including worker panics, remain logged no-edits responses.
        match self
            .dispatch("formatting", move |snapshot| {
                handlers::documents::formatting(snapshot, params)
            })
            .await
        {
            Ok(edits) => Ok(edits),
            Err(err) => {
                error!(?err, "formatter worker failed");
                Ok(None)
            }
        }
    }
}
