//! Thin asynchronous transport adapter over synchronous snapshot handlers.
use crate::{
    handlers::{self, HandlerError},
    perf,
    state::{Snapshot, Workspace},
};
use lsp_types::*;
use parking_lot::Mutex;
use pkl_db::settings::WorkspaceSettings;
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
            code: match error {
                HandlerError::InvalidParams(_) => jsonrpc::ErrorCode::InvalidParams,
                HandlerError::ContentModified => jsonrpc::ErrorCode::ServerError(-32801),
                HandlerError::Domain(_) => jsonrpc::ErrorCode::InternalError,
            },
            message: error.to_string().into(),
            data: None,
        }
    }
}
#[derive(Clone, Copy)]
enum JobScope {
    Analysis,
    Projects,
}

#[derive(Debug)]
pub struct PklBackend {
    pub workspace: Arc<Mutex<Workspace>>,
    pub client: Option<Client>,
    diagnostic_workers: Mutex<HashMap<Uri, watch::Sender<Option<u64>>>>,
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
        perf::blocking(stage, move || snapshot.run(stage, handler))
            .await
            .map_err(HandlerError::from)?
            .map_err(Into::into)
    }
    async fn external<J: Send + 'static, R: Send + 'static>(
        &self,
        stage: &'static str,
        scope: JobScope,
        prepare: impl FnOnce(&Snapshot) -> handlers::Result<J> + Send + 'static,
        execute: impl FnOnce(J) -> handlers::Result<R> + Send + 'static,
    ) -> jsonrpc::Result<R> {
        let (generation, job) = self
            .dispatch(stage, move |snapshot| {
                let generation = match scope {
                    JobScope::Analysis => snapshot.generation,
                    JobScope::Projects => snapshot.project_generation,
                };
                prepare(snapshot).map(|job| (generation, job))
            })
            .await?;
        let result = perf::blocking(stage, move || execute(job)).await;
        let current = {
            let workspace = self.workspace.lock();
            match scope {
                JobScope::Analysis => workspace.generation(),
                JobScope::Projects => workspace.project_generation(),
            }
        };
        if current != generation {
            perf::cancelled(stage);
            return Err(HandlerError::ContentModified.into());
        }
        result.map_err(HandlerError::from)?.map_err(Into::into)
    }

    async fn reload(&self, work: crate::state::ReloadWork) {
        match perf::blocking("load_sources", move || work.load()).await {
            Ok(loaded) => self.workspace.lock().apply_reload(loaded),
            Err(error) => error!(?error, "source refresh failed"),
        }
        self.schedule_diagnostics();
    }
    async fn restore_workspaces(&self) -> jsonrpc::Result<()> {
        let manager = self
            .external(
                "restore_workspaces",
                JobScope::Projects,
                |snapshot| Ok(handlers::lifecycle::prepare_restore_workspaces(snapshot)),
                handlers::lifecycle::restore_workspaces,
            )
            .await?;
        self.workspace.lock().apply_project_manager(manager);
        Ok(())
    }

    /// Watch slots hold generation numbers, never idle Salsa handles: writes
    /// must be able to cancel running computations and wait for their drop.
    fn schedule_diagnostics(&self) {
        let Some(client) = &self.client else { return };
        let current = self.workspace.lock();
        let generation = current.generation();
        let mut workers = self.diagnostic_workers.lock();
        workers.retain(|uri, sender| {
            if !current.is_document_open(uri) {
                sender.send_replace(None);
                false
            } else {
                true
            }
        });
        for uri in current.open_document_uris() {
            if let Some(sender) = workers.get(&uri) {
                sender.send_replace(Some(generation));
                continue;
            }
            let (sender, mut receiver) = watch::channel(Some(generation));
            workers.insert(uri.clone(), sender);
            let workspace = Arc::clone(&self.workspace);
            let client = client.clone();
            tokio::spawn(async move {
                loop {
                    let Some(generation) = *receiver.borrow_and_update() else {
                        break;
                    };
                    let snapshot = workspace.lock().snapshot();
                    if snapshot.generation != generation {
                        drop(snapshot);
                        if receiver.changed().await.is_err() {
                            break;
                        }
                        continue;
                    }
                    let analysis_uri = uri.clone();
                    let result = perf::blocking("diagnostics", move || {
                        snapshot.run("diagnostics", |snapshot| {
                            let version = snapshot.document_version(analysis_uri.as_str());
                            handlers::diagnostics::diagnostics(snapshot, analysis_uri.as_str())
                                .map(|diagnostics| (diagnostics, version))
                        })
                    })
                    .await;
                    match receiver.has_changed() {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(_) => break,
                    }
                    let valid = {
                        let latest = workspace.lock();
                        latest.generation() == generation && latest.is_document_open(&uri)
                    };
                    if valid {
                        match result {
                            Ok(Ok((diagnostics, version))) => {
                                tokio::select! {
                                    biased;
                                    changed = receiver.changed() => { if changed.is_err() { break; } continue; }
                                    _ = client.publish_diagnostics(uri.clone(), diagnostics, version) => {}
                                }
                            }
                            Ok(Err(HandlerError::ContentModified)) => {}
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
    async fn refresh_analysis(&self) {
        let work = {
            let mut workspace = self.workspace.lock();
            workspace.refresh_projects();
            workspace.prepare_refresh()
        };
        self.reload(work).await;
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
                        self.refresh_analysis().await;
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
        let contents = self
            .external(
                "file_contents",
                JobScope::Analysis,
                move |snapshot| handlers::documents::prepare_file_contents(snapshot, document),
                handlers::documents::file_contents,
            )
            .await?;
        if contents.loaded {
            self.workspace
                .lock()
                .apply_loaded_source(Arc::clone(&contents.source))
                .map_err(HandlerError::from)?;
            self.schedule_diagnostics();
        }
        Ok(contents.source.contents().to_owned())
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "sync_projects"))]
    pub async fn sync_projects(&self) -> jsonrpc::Result<()> {
        let manager = self
            .external(
                "sync_projects",
                JobScope::Projects,
                |snapshot| Ok(handlers::commands::prepare_sync_projects(snapshot)),
                handlers::commands::execute_command,
            )
            .await?;
        if let Some(manager) = manager {
            self.workspace.lock().apply_project_manager(manager);
        }
        self.refresh_analysis().await;
        Ok(())
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "download_package"))]
    pub async fn download_package(&self, uri: String) -> jsonrpc::Result<()> {
        self.external(
            "download_package",
            JobScope::Projects,
            move |snapshot| handlers::commands::prepare_download_package(snapshot, uri),
            handlers::commands::execute_command,
        )
        .await?;
        self.refresh_analysis().await;
        Ok(())
    }
}
impl LanguageServer for PklBackend {
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "initialize"))]
    async fn initialize(&self, params: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        let _perf = perf::Handler::enter();
        let result = handlers::lifecycle::initialize(&mut self.workspace.lock(), &params);
        self.restore_workspaces().await?;
        Ok(result)
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "initialized"))]
    async fn initialized(&self, _: InitializedParams) {
        let _perf = perf::Handler::enter();
        info!("server initialized");
        let watched_files = self.workspace.lock().watched_files;
        if watched_files && let Some(client) = &self.client {
            let watchers = ["**/*.pkl", "**/PklProject", "**/PklProject.deps.json"]
                .into_iter()
                .map(|pattern| FileSystemWatcher {
                    glob_pattern: GlobPattern::String(pattern.into()),
                    kind: Some(WatchKind::Create | WatchKind::Change | WatchKind::Delete),
                })
                .collect();
            let options = DidChangeWatchedFilesRegistrationOptions { watchers };
            let registration = Registration {
                id: "pkl-watched-files".into(),
                method: "workspace/didChangeWatchedFiles".into(),
                register_options: Some(
                    serde_json::to_value(options).expect("watcher options are serializable"),
                ),
            };
            if let Err(error) = client.register_capability(vec![registration]).await {
                error!(?error, "could not register file watchers");
            }
        }
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
        let uri = params.text_document.uri.clone();
        let work = match handlers::lifecycle::did_close(&mut self.workspace.lock(), params) {
            Ok(work) => work,
            Err(error) => {
                error!(?error, "didClose failed");
                return;
            }
        };
        if let Some(sender) = self.diagnostic_workers.lock().remove(&uri) {
            sender.send_replace(None);
        }
        self.reload(work).await;
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "did_save"))]
    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let _perf = perf::Handler::enter();
        if let Err(err) = handlers::lifecycle::did_save(&mut self.workspace.lock(), params) {
            error!(?err, "didSave failed");
        }
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "did_change_watched_files"))]
    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let _perf = perf::Handler::enter();
        let work = self.workspace.lock().prepare_watched_files(params);
        self.reload(work).await;
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
            self.refresh_analysis().await;
        }
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "did_change_workspace_folders"))]
    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        let _perf = perf::Handler::enter();
        handlers::lifecycle::did_change_workspace_folders(&mut self.workspace.lock(), params);
        if let Err(error) = self.restore_workspaces().await {
            error!(?error, "project state restoration failed");
        }
        self.refresh_configuration().await;
        self.schedule_diagnostics();
    }
    #[tracing::instrument(target = "lspkl::perf", level = "debug", skip_all, fields(handler_id = tracing::field::Empty, method = "execute_command"))]
    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> jsonrpc::Result<Option<serde_json::Value>> {
        let manager = self
            .external(
                "execute_command",
                JobScope::Projects,
                move |snapshot| handlers::commands::prepare_execute_command(snapshot, params),
                handlers::commands::execute_command,
            )
            .await?;
        if let Some(manager) = manager {
            self.workspace.lock().apply_project_manager(manager);
        }
        self.refresh_analysis().await;
        Ok(None)
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
            .external(
                "formatting",
                JobScope::Analysis,
                move |snapshot| handlers::documents::prepare_formatting(snapshot, params),
                |request| {
                    request
                        .map(handlers::documents::formatting)
                        .transpose()
                        .map(Option::flatten)
                },
            )
            .await
        {
            Ok(edits) => Ok(edits),
            Err(error) if error.code == jsonrpc::ErrorCode::ServerError(-32801) => Err(error),
            Err(err) => {
                error!(?err, "formatter worker failed");
                Ok(None)
            }
        }
    }
}
