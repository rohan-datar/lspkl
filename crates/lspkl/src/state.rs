//! Notification-side workspace ownership and short-lived Salsa request handles.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use fluent_uri::Uri;
use lspkl_db::{
    Db, DirectoryUpdate, File, FileStatus, IoUpdate, RootDatabase, settings::WorkspaceSettings,
};
use lspkl_features::positions::PositionEncoding;
use lspkl_project::{PklProject, cli::PklCli, dto::ProjectDeps};
use lspkl_vfs::{
    FileError, Source, SourceLoader, StandardSourceLoader, parse_source_uri, url_to_path,
};
use salsa::{Database, Setter};

use crate::{
    handlers::{HandlerError, Result},
    perf,
};

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error(transparent)]
    Source(#[from] FileError),
    #[error("file is not open: {0}")]
    NotOpen(Uri<String>),
}

/// Owns the writable database. No idle database clones may survive an input write.
pub struct Workspace {
    pub(crate) db: RootDatabase,
    https_loader: Arc<StandardSourceLoader>,
    project_manager: Arc<parking_lot::Mutex<lspkl_project::project_manager::ProjectManager>>,
    settings: Arc<WorkspaceSettings>,
    explicit_settings: WorkspaceSettings,
    position_encoding: PositionEncoding,
    client_configuration: bool,
    pub watched_files: bool,
    generation: u64,
    project_generation: u64,
    next_reload: u64,
    // Newer source reads inherit pending watched-project invalidation intent.
    pending_reloads: HashMap<Uri<String>, (u64, bool)>,
    // Editor identities are distinct even when they resolve to the same Salsa input.
    open_documents: HashMap<lsp_types::Uri, Uri<String>>,
    directory_reload: u64,
}

/// A database clone owned only by an actively running request or worker.
#[derive(Clone)]
pub struct Snapshot {
    pub db: RootDatabase,
    pub(crate) project_manager:
        Arc<parking_lot::Mutex<lspkl_project::project_manager::ProjectManager>>,
    pub settings: Arc<WorkspaceSettings>,
    pub position_encoding: PositionEncoding,
    pub client_configuration: bool,
    pub generation: u64,
    pub project_generation: u64,
}

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Snapshot")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}
impl Default for Workspace {
    fn default() -> Self {
        let https_loader = Arc::new(StandardSourceLoader::new());
        let db = RootDatabase::new(https_loader.clone(), WorkspaceSettings::default());
        Self::with_database_and_https_loader(db, https_loader)
    }
}
impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }
    /// Bootstrap immutable SDK analysis before exposing any request snapshots.
    pub fn with_database(db: RootDatabase) -> Self {
        Self::with_database_and_https_loader(db, Arc::new(StandardSourceLoader::new()))
    }
    /// Retain a configured HTTPS loader, including its trusted certificate roots.
    pub fn with_database_and_https_loader(
        db: RootDatabase,
        https_loader: Arc<StandardSourceLoader>,
    ) -> Self {
        let _ = lspkl_types::base_module(&db);
        let settings = Arc::new(db.config().settings(&db).clone());
        Self {
            db,
            https_loader,
            project_manager: Arc::new(parking_lot::Mutex::new(Default::default())),
            explicit_settings: (*settings).clone(),
            settings,
            position_encoding: PositionEncoding::default(),
            client_configuration: false,
            watched_files: false,
            generation: 0,
            project_generation: 0,
            next_reload: 0,
            pending_reloads: HashMap::new(),
            open_documents: HashMap::new(),
            directory_reload: 0,
        }
    }
    /// No parsing, I/O, traversal, or text copying occurs during capture.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            db: self.db.clone(),
            project_manager: Arc::clone(&self.project_manager),
            settings: Arc::clone(&self.settings),
            position_encoding: self.position_encoding,
            client_configuration: self.client_configuration,
            generation: self.generation,
            project_generation: self.project_generation,
        }
    }
    pub fn document_version(&self, uri: &str) -> Option<i32> {
        self.document(uri).and_then(|file| file.version(&self.db))
    }
    pub fn project_manager(&self) -> lspkl_project::project_manager::ProjectManager {
        self.project_manager.lock().clone()
    }
    pub fn settings(&self) -> &WorkspaceSettings {
        &self.settings
    }
    pub fn client_configuration(&self) -> bool {
        self.client_configuration
    }
    pub fn database(&self) -> &RootDatabase {
        &self.db
    }
    /// Owned I/O handles; retaining these does not retain a Salsa database snapshot.
    pub fn semantic_loaders(
        &self,
    ) -> (
        Arc<dyn SourceLoader + Send + Sync>,
        Arc<StandardSourceLoader>,
    ) {
        (
            self.db.source_loader_handle(),
            Arc::clone(&self.https_loader),
        )
    }
    pub fn apply_io(&mut self, update: IoUpdate) -> std::result::Result<(), FileError> {
        let _write = perf::Compute::runtime("salsa_write");
        self.db.apply_io(update)?;
        // Missing results complete an attempt and must also refresh diagnostics.
        self.invalidate_analysis();
        Ok(())
    }
    pub fn request_routing_refresh(&mut self) {
        self.db.request_routing_refresh();
    }
    pub(crate) fn project_generation(&self) -> u64 {
        self.project_generation
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn invalidate_analysis(&mut self) {
        self.generation += 1;
    }
    pub fn document(&self, uri: &str) -> Option<File> {
        document(&self.db, uri)
    }
    pub fn open_document_uris(&self) -> Vec<lsp_types::Uri> {
        self.open_documents.keys().cloned().collect()
    }
    pub(crate) fn is_document_open(&self, uri: &lsp_types::Uri) -> bool {
        self.open_documents.contains_key(uri)
    }
    pub fn apply_open(
        &mut self,
        params: lsp_types::DidOpenTextDocumentParams,
    ) -> std::result::Result<(), FileError> {
        let document = params.text_document;
        let uri = parse_source_uri(document.uri.as_str())?;
        let _write = perf::Compute::runtime("salsa_write");
        self.db
            .open_file(&uri, Arc::from(document.text), Some(document.version))?;
        self.open_documents.insert(document.uri, uri);
        self.invalidate_analysis();
        Ok(())
    }
    pub fn apply_change(
        &mut self,
        params: lsp_types::DidChangeTextDocumentParams,
    ) -> std::result::Result<(), WorkspaceError> {
        let uri = parse_source_uri(params.text_document.uri.as_str())?;
        if !self
            .db
            .known_file(&uri)
            .is_some_and(|file| file.status(&self.db) == FileStatus::Open)
        {
            return Err(WorkspaceError::NotOpen(uri));
        }
        let Some(change) = params.content_changes.into_iter().last() else {
            return Ok(());
        };
        let _write = perf::Compute::runtime("salsa_write");
        self.db.open_file(
            &uri,
            Arc::from(change.text),
            Some(params.text_document.version),
        )?;
        self.invalidate_analysis();
        Ok(())
    }
    /// End editor ownership immediately; backing I/O runs without a database handle.
    pub fn apply_close(
        &mut self,
        params: lsp_types::DidCloseTextDocumentParams,
    ) -> std::result::Result<ReloadWork, FileError> {
        let uri = parse_source_uri(params.text_document.uri.as_str())?;
        self.open_documents.remove(&params.text_document.uri);
        if self
            .open_documents
            .values()
            .any(|open_uri| open_uri == &uri)
        {
            self.invalidate_analysis();
            return Ok(self.reload_work(Vec::new(), false, false));
        }
        if let Some(file) = self
            .db
            .known_file(&uri)
            .filter(|file| file.status(&self.db) == FileStatus::Open)
        {
            let _write = perf::Compute::runtime("salsa_write");
            let status = if uri.scheme().as_str() == "pkl" {
                FileStatus::Present
            } else {
                FileStatus::Missing
            };
            file.set_status(&mut self.db).to(status);
            file.set_version(&mut self.db).to(None);
            self.invalidate_analysis();
        }
        Ok(self.reload_work(vec![uri], false, false))
    }
    /// Explicit sources replace the fallback used by subsequent configuration pulls.
    pub fn apply_explicit_settings(&mut self, settings: WorkspaceSettings) {
        self.explicit_settings = settings.clone();
        self.apply_settings(settings);
    }
    /// A null pull section means no client value, not an explicit reset.
    pub fn apply_pulled_settings(
        &mut self,
        values: &[serde_json::Value; 5],
    ) -> std::result::Result<(), serde_json::Error> {
        let mut settings = self.explicit_settings.clone();
        if !values[0].is_null() {
            settings.cli.path = serde_json::from_value(values[0].clone())?;
        }
        if !values[1].is_null() {
            settings.formatter.grammar_version = serde_json::from_value(values[1].clone())?;
        }
        if !values[2].is_null() {
            settings.modulepath = serde_json::from_value(values[2].clone())?;
        }
        if !values[3].is_null() {
            settings.projects.excluded_directories = serde_json::from_value(values[3].clone())?;
        }
        if !values[4].is_null() {
            settings.cache_dir = serde_json::from_value(values[4].clone())?;
        }
        settings.module_paths.clear();
        settings.resolved_cache_dir = None;
        self.apply_settings(settings);
        Ok(())
    }
    pub fn apply_settings(&mut self, mut settings: WorkspaceSettings) {
        settings.working_directory = self.settings.working_directory.clone();
        // Provisional path binding is pure; existence-based selection is an owned
        // I/O job and feeds its resolved settings back through this setter.
        let folders = self.project_manager.lock().workspace_folders().to_vec();
        if settings.module_paths.len() != settings.modulepath.len() {
            settings.module_paths = settings
                .modulepath
                .iter()
                .map(|path| {
                    if path.is_absolute() {
                        path.clone()
                    } else {
                        folders
                            .first()
                            .unwrap_or(&settings.working_directory)
                            .join(path)
                    }
                })
                .collect();
        }
        if settings.resolved_cache_dir.is_none() {
            settings.resolved_cache_dir = settings.cache_dir.as_ref().map(|path| {
                if path.is_absolute() {
                    path.clone()
                } else {
                    folders
                        .first()
                        .unwrap_or(&settings.working_directory)
                        .join(path)
                }
            });
        }
        let cli = PklCli {
            binary_path: settings.cli.path.clone(),
        };
        {
            let mut projects = self.project_manager.lock();
            projects.configure(cli.clone(), settings.projects.excluded_directories.clone());
        }
        if *self.settings == settings {
            return;
        }
        self.project_generation += 1;
        let _write = perf::Compute::runtime("salsa_write");
        self.db.set_settings(settings.clone());
        self.db.request_routing_refresh();
        self.settings = Arc::new(settings);
        self.invalidate_analysis();
    }
    pub fn apply_initialize(&mut self, params: &lsp_types::InitializeParams) {
        self.position_encoding = if params
            .capabilities
            .general
            .as_ref()
            .and_then(|caps| caps.position_encodings.as_ref())
            .is_some_and(|encodings| encodings.contains(&lsp_types::PositionEncodingKind::UTF8))
        {
            PositionEncoding::Utf8
        } else {
            PositionEncoding::Utf16
        };
        self.client_configuration = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|caps| caps.configuration)
            .unwrap_or(false);
        self.watched_files = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|caps| caps.did_change_watched_files.as_ref())
            .and_then(|caps| caps.dynamic_registration)
            .unwrap_or(false);
        #[allow(deprecated)]
        let uris: Vec<_> = params
            .workspace_folders
            .as_ref()
            .map(|folders| folders.iter().map(|folder| &folder.uri).collect())
            .unwrap_or_else(|| params.root_uri.iter().collect());
        let folders = uris
            .into_iter()
            .filter_map(|uri| parse_source_uri(uri.as_str()).ok())
            .filter_map(|uri| url_to_path(&uri).ok())
            .collect();
        self.project_manager.lock().initialize(folders);
        match params
            .initialization_options
            .clone()
            .map(WorkspaceSettings::parse)
        {
            Some(Ok(settings)) => self.apply_explicit_settings(settings),
            Some(Err(error)) => tracing::error!(?error, "invalid initialization settings"),
            None => {}
        }
    }
    pub fn apply_workspace_folders(&mut self, params: lsp_types::DidChangeWorkspaceFoldersParams) {
        // Folder arrival supersedes jobs captured against the previous roots.
        self.project_generation += 1;
        let mut folders = self.project_manager.lock().workspace_folders().to_vec();
        for folder in params.event.removed {
            if let Ok(uri) = parse_source_uri(folder.uri.as_str())
                && let Ok(path) = url_to_path(&uri)
            {
                folders.retain(|existing| existing != &path);
            }
        }
        for folder in params.event.added {
            if let Ok(uri) = parse_source_uri(folder.uri.as_str())
                && let Ok(path) = url_to_path(&uri)
                && !folders.contains(&path)
            {
                folders.push(path);
            }
        }
        self.project_manager.lock().initialize(folders);
        self.refresh_projects();
        let mut settings = (*self.settings).clone();
        settings.module_paths.clear();
        settings.resolved_cache_dir = None;
        self.apply_settings(settings);
    }
    pub fn apply_project_manager(
        &mut self,
        manager: lspkl_project::project_manager::ProjectManager,
    ) {
        self.project_generation += 1;
        self.project_manager = Arc::new(parking_lot::Mutex::new(manager));
        self.refresh_projects();
    }
    pub fn refresh_projects(&mut self) {
        let projects = self.project_manager.lock().owned_projects();
        let _write = perf::Compute::runtime("salsa_write");
        self.db.set_projects(projects);
        self.invalidate_analysis();
    }
    pub fn prepare_watched_files(
        &mut self,
        params: lsp_types::DidChangeWatchedFilesParams,
    ) -> ReloadWork {
        let mut uris: Vec<_> = params
            .changes
            .into_iter()
            .filter_map(|event| parse_source_uri(event.uri.as_str()).ok())
            .collect();
        if uris
            .iter()
            .filter_map(|uri| url_to_path(uri).ok())
            .any(|path| {
                matches!(
                    path.file_name().and_then(|name| name.to_str()),
                    Some("PklProject" | "PklProject.deps.json")
                )
            })
        {
            self.project_generation += 1;
        }
        let changed = uris.len();
        for file in self.db.all_files() {
            let uri = file.uri(&self.db);
            if uri.scheme().as_str() != "jar" {
                continue;
            }
            if let Ok(archive) = lspkl_vfs::archive_file_uri(uri)
                && uris[..changed].contains(&archive)
            {
                uris.push(uri.clone());
            }
        }
        uris.sort();
        uris.dedup();
        self.reload_work(uris, true, true)
    }
    pub fn prepare_refresh(&mut self) -> ReloadWork {
        let uris = self
            .db
            .all_files()
            .into_iter()
            .filter(|file| {
                file.status(&self.db) != FileStatus::Open
                    && file.uri(&self.db).scheme().as_str() != "pkl"
            })
            .map(|file| file.uri(&self.db).clone())
            .collect();
        self.reload_work(uris, true, false)
    }
    fn reload_work(
        &mut self,
        uris: Vec<Uri<String>>,
        directories: bool,
        watched_projects: bool,
    ) -> ReloadWork {
        self.next_reload += 1;
        let token = self.next_reload;
        let uris = uris
            .into_iter()
            .map(|uri| {
                let pending = self
                    .pending_reloads
                    .entry(uri.clone())
                    .or_insert((token, false));
                *pending = (token, pending.1 || watched_projects);
                (uri, pending.1)
            })
            .collect();
        let roots = if directories {
            self.directory_reload = token;
            self.db.directory_roots()
        } else {
            Vec::new()
        };
        ReloadWork {
            uris,
            token,
            roots,
            loader: self.db.source_loader_handle(),
            cache_dir: self.settings.effective_cache_dir().to_owned(),
        }
    }
    pub fn apply_reload(&mut self, loaded: LoadedFiles) {
        if loaded.updates.is_empty() && loaded.directories.is_empty() {
            return;
        }
        let _write = perf::Compute::runtime("salsa_write");
        let mut project_changed = false;
        for update in loaded.updates {
            if self
                .pending_reloads
                .get(&update.uri)
                .map(|pending| pending.0)
                != Some(loaded.token)
            {
                continue;
            }
            self.pending_reloads.remove(&update.uri);
            if let Some(path) = update.project_path {
                project_changed |= self
                    .project_manager
                    .lock()
                    .apply_watched_file(&path, update.deps);
            }
            if let Err(error) = self.db.apply_source(
                &update.uri,
                update.source.ok().map(|source| Arc::clone(&source.text)),
            ) {
                tracing::error!(?error, "could not install watched source");
            }
        }
        if loaded.token == self.directory_reload {
            self.db.apply_directory_updates(loaded.directories);
        }
        if project_changed {
            self.project_generation += 1;
            let projects = self.project_manager.lock().owned_projects();
            self.db.set_projects(projects);
        }
        self.invalidate_analysis();
    }
    pub fn apply_loaded_source(
        &mut self,
        source: Arc<Source>,
    ) -> std::result::Result<(), FileError> {
        let _write = perf::Compute::runtime("salsa_write");
        let uri = source.url.clone();
        self.db.apply_source(&uri, Some(Arc::clone(&source.text)))?;
        self.invalidate_analysis();
        Ok(())
    }
}

fn document(db: &RootDatabase, uri: &str) -> Option<File> {
    let uri = parse_source_uri(uri).ok()?;
    let file = db.known_file(&uri)?;
    (file.status(db) == FileStatus::Open).then_some(file)
}
impl Snapshot {
    pub fn document(&self, uri: &str) -> Option<File> {
        document(&self.db, uri)
    }
    pub fn document_version(&self, uri: &str) -> Option<i32> {
        self.document(uri).and_then(|file| file.version(&self.db))
    }
    /// Catch only Salsa cancellation; unrelated panics remain ordinary worker failures.
    pub fn run<R>(self, stage: &'static str, work: impl FnOnce(&Self) -> Result<R>) -> Result<R> {
        salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
            self.db.unwind_if_revision_cancelled();
            let result = work(&self);
            self.db.unwind_if_revision_cancelled();
            result
        }))
        .unwrap_or_else(|_| {
            perf::cancelled(stage);
            Err(HandlerError::ContentModified)
        })
    }
}

/// No Salsa handles: slow reads cannot hold up input writes or editor notifications.
pub struct ReloadWork {
    uris: Vec<(Uri<String>, bool)>,
    token: u64,
    roots: Vec<Uri<String>>,
    loader: Arc<dyn SourceLoader + Send + Sync>,
    cache_dir: PathBuf,
}
pub struct LoadedFiles {
    token: u64,
    updates: Vec<LoadedFile>,
    directories: Vec<DirectoryUpdate>,
}
struct LoadedFile {
    uri: Uri<String>,
    source: std::result::Result<Arc<Source>, FileError>,
    project_path: Option<PathBuf>,
    deps: Option<ProjectDeps>,
}
impl ReloadWork {
    pub fn load(self) -> LoadedFiles {
        let updates = self
            .uris
            .into_iter()
            .map(|(uri, watched_projects)| {
                // Only explicit watcher events invalidate derived project metadata.
                // Closing a document or refreshing sources must retain synced routing.
                let project_path = watched_projects
                    .then(|| url_to_path(&uri).ok())
                    .flatten()
                    .filter(|path| {
                        matches!(
                            path.file_name().and_then(|name| name.to_str()),
                            Some("PklProject" | "PklProject.deps.json")
                        )
                    });
                let source = self.loader.load(&uri, &self.cache_dir);
                let deps = if project_path.as_ref().is_some_and(|path| {
                    path.file_name()
                        .is_some_and(|name| name == "PklProject.deps.json")
                }) {
                    source.as_ref().ok().and_then(|source| {
                        match PklProject::load_project_deps(source.contents()) {
                            Ok(deps) => Some(deps),
                            Err(error) => {
                                tracing::error!(?error, "invalid watched project dependencies");
                                None
                            }
                        }
                    })
                } else {
                    None
                };
                LoadedFile {
                    uri,
                    source,
                    project_path,
                    deps,
                }
            })
            .collect();
        let directories = self
            .roots
            .iter()
            .map(|root| DirectoryUpdate::load(&*self.loader, root))
            .collect();
        LoadedFiles {
            token: self.token,
            updates,
            directories,
        }
    }
}
