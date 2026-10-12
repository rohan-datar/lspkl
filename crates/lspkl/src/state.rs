//! Process-wide state shared across LSP handlers.

use std::collections::{HashMap, hash_map::Entry};
use std::sync::{Arc, LazyLock};

use lsp_features::positions::PositionEncoding;
use pkl_diagnostics::DiagnosticsEngine;
use pkl_project::PklProject;
use pkl_project::cli::PklCli;
use pkl_project::import_resolver::ImportResolver;
use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};
use pkl_types::{BaseModule, FileId, ImportResolution, NodeRef};
use pkl_vfs::{FileError, FileManager, VirtualFile, url_to_path};

use crate::file_registry::FileRegistry;
use crate::settings::WorkspaceSettings;

/// A resolved file paired with the immutable syntax snapshot used for analysis.
pub struct ParsedModule {
    pub file_id: FileId,
    pub file: Arc<VirtualFile>,
    pub tree: Arc<SyntaxTree>,
}

/// Snapshot used for analysis during a single request
pub struct AnalysisSnapshot {
    pub module: ParsedModule,
    pub trees: HashMap<FileId, Arc<SyntaxTree>>,
    pub imports: HashMap<NodeRef, ImportResolution>,
}

type PendingModule = (FileId, Arc<VirtualFile>, Option<Arc<PklProject>>);

/// Exclusive notification-side workspace owner. Requests use immutable snapshots.
#[derive(Debug)]
pub struct Workspace {
    current: Snapshot,
    buffers: Arc<HashMap<fluent_uri::Uri<String>, Arc<VirtualFile>>>,
}

/// Cheap request capture. Open files and settings retain their captured epoch;
/// loader/project/package caches remain shared until the later database cutover.
#[derive(Clone)]
pub struct Snapshot {
    pub file_manager: Arc<FileManager>,
    pub file_registry: Arc<parking_lot::Mutex<FileRegistry>>,
    base_module: Arc<LazyLock<BaseModule>>,
    diagnostics: Arc<LazyLock<DiagnosticsEngine>>,
    pub project_manager: Arc<parking_lot::Mutex<pkl_project::project_manager::ProjectManager>>,
    pub package_manager: Arc<parking_lot::Mutex<pkl_project::package_manager::PackageManager>>,
    pub settings: Arc<WorkspaceSettings>,
    pub client_configuration: bool,
    pub position_encoding: PositionEncoding,
    pub generation: u64,
    pub open_documents: Arc<HashMap<FileId, Arc<lsp_types::Uri>>>,
    buffers: Arc<HashMap<fluent_uri::Uri<String>, Arc<VirtualFile>>>,
}

impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Snapshot")
            .field("generation", &self.generation)
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl Default for Workspace {
    fn default() -> Self {
        let settings = Arc::new(WorkspaceSettings::default());
        let buffers = Arc::new(HashMap::new());
        let file_manager = Arc::new(FileManager::new().with_open_documents(
            Arc::clone(&buffers),
            settings.effective_cache_dir().to_owned(),
        ));
        Self {
            buffers: Arc::clone(&buffers),
            current: Snapshot {
                file_manager,
                file_registry: Arc::new(parking_lot::Mutex::new(FileRegistry::default())),
                base_module: Arc::new(LazyLock::new(|| {
                    BaseModule::load_embedded().expect("embedded pkl:base must load")
                })),
                diagnostics: Arc::new(LazyLock::new(DiagnosticsEngine::default)),
                project_manager: Arc::new(parking_lot::Mutex::new(Default::default())),
                package_manager: Arc::new(parking_lot::Mutex::new(
                    pkl_project::package_manager::PackageManager::new(Default::default()),
                )),
                settings,
                client_configuration: false,
                position_encoding: PositionEncoding::default(),
                generation: 0,
                open_documents: Arc::new(HashMap::new()),
                buffers,
            },
        }
    }
}

impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }

    /// No parsing, I/O, map traversal, or source copies occurs during capture.
    pub fn snapshot(&self) -> Snapshot {
        self.current.clone()
    }

    pub fn invalidate_analysis(&mut self) {
        self.current.generation += 1;
    }

    fn refresh_buffers(&mut self) {
        self.current.buffers = Arc::clone(&self.buffers);
        self.current.file_manager = Arc::new(self.current.file_manager.with_open_documents(
            Arc::clone(&self.buffers),
            self.current.settings.effective_cache_dir().to_owned(),
        ));
        self.invalidate_analysis();
    }

    pub fn apply_open(
        &mut self,
        params: lsp_types::DidOpenTextDocumentParams,
    ) -> Result<(), FileError> {
        let document = params.text_document;
        let url =
            pkl_vfs::effective_uri(&fluent_uri::Uri::parse(document.uri.as_str().to_owned())?)?;
        let origin = match url.scheme().as_str() {
            "file" => pkl_vfs::Origin::Fs { url: url.clone() },
            "pkl" => pkl_vfs::Origin::Stdlib,
            "https" => pkl_vfs::Origin::Https,
            "jar" => pkl_vfs::Origin::Jar,
            _ => pkl_vfs::Origin::Ephemeral,
        };
        let file = Arc::new(VirtualFile::new(
            url.clone(),
            origin,
            document.text,
            Some(document.version),
        ));
        let id = self.current.file_id(&file);
        Arc::make_mut(&mut self.buffers).insert(url, file);
        Arc::make_mut(&mut self.current.open_documents).insert(id, Arc::new(document.uri));
        self.refresh_buffers();
        Ok(())
    }

    pub fn apply_change(
        &mut self,
        params: lsp_types::DidChangeTextDocumentParams,
    ) -> Result<(), FileError> {
        let url = pkl_vfs::effective_uri(&fluent_uri::Uri::parse(
            params.text_document.uri.as_str().to_owned(),
        )?)?;
        let Some(file) = self.buffers.get(&url) else {
            return Err(FileError::NotOpen(url));
        };
        let Some(change) = params.content_changes.into_iter().last() else {
            return Ok(());
        };
        let updated = Arc::new(VirtualFile::new(
            url.clone(),
            file.origin.clone(),
            change.text,
            Some(params.text_document.version),
        ));
        Arc::make_mut(&mut self.buffers).insert(url, updated);
        self.refresh_buffers();
        Ok(())
    }

    pub fn apply_close(
        &mut self,
        params: lsp_types::DidCloseTextDocumentParams,
    ) -> Result<(), FileError> {
        let url = pkl_vfs::effective_uri(&fluent_uri::Uri::parse(
            params.text_document.uri.as_str().to_owned(),
        )?)?;
        // A pre-open disk entry may predate a save made while this buffer was open.
        self.current.file_manager.close(&url);
        if let Some(file) = Arc::make_mut(&mut self.buffers).remove(&url) {
            let id = self.current.file_id(&file);
            Arc::make_mut(&mut self.current.open_documents).remove(&id);
            self.refresh_buffers();
        }
        Ok(())
    }

    // Full-sync save does not replace the editor's versioned buffer.
    pub fn apply_save(
        &mut self,
        _params: lsp_types::DidSaveTextDocumentParams,
    ) -> Result<(), FileError> {
        Ok(())
    }

    pub fn apply_settings(&mut self, mut settings: WorkspaceSettings) {
        let cli = PklCli {
            binary_path: settings.cli.path.clone(),
        };
        {
            let mut projects = self.current.project_manager.lock();
            settings.resolve_paths(projects.workspace_folders());
            projects.configure(cli.clone(), settings.projects.excluded_directories.clone());
        }
        self.current.package_manager.lock().set_cli(cli);
        self.current.settings = Arc::new(settings);
        self.refresh_buffers();
    }

    pub fn apply_initialize(&mut self, params: &lsp_types::InitializeParams) {
        self.current.position_encoding = if params
            .capabilities
            .general
            .as_ref()
            .and_then(|general| general.position_encodings.as_ref())
            .is_some_and(|encodings| encodings.contains(&lsp_types::PositionEncodingKind::UTF8))
        {
            PositionEncoding::Utf8
        } else {
            PositionEncoding::Utf16
        };
        self.current.client_configuration = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|caps| caps.configuration)
            .unwrap_or(false);
        #[allow(deprecated)]
        let uris: Vec<_> = params
            .workspace_folders
            .as_ref()
            .map(|folders| folders.iter().map(|folder| &folder.uri).collect())
            .unwrap_or_else(|| params.root_uri.iter().collect());
        let folders = uris
            .into_iter()
            .filter_map(|uri| fluent_uri::Uri::parse(uri.as_str().to_owned()).ok())
            .filter_map(|uri| url_to_path(&uri).ok())
            .collect();
        self.current.project_manager.lock().initialize(folders);
        match params
            .initialization_options
            .clone()
            .map(WorkspaceSettings::parse)
        {
            Some(Ok(settings)) => self.apply_settings(settings),
            Some(Err(error)) => tracing::error!(?error, "invalid initialization settings"),
            None => {}
        }
    }

    pub fn apply_workspace_folders(&mut self, params: lsp_types::DidChangeWorkspaceFoldersParams) {
        let mut folders = self
            .current
            .project_manager
            .lock()
            .workspace_folders()
            .to_vec();
        for folder in params.event.removed {
            if let Ok(uri) = fluent_uri::Uri::parse(folder.uri.as_str().to_owned())
                && let Ok(path) = url_to_path(&uri)
            {
                folders.retain(|existing| existing != &path);
            }
        }
        for folder in params.event.added {
            if let Ok(uri) = fluent_uri::Uri::parse(folder.uri.as_str().to_owned())
                && let Ok(path) = url_to_path(&uri)
                && !folders.contains(&path)
            {
                folders.push(path);
            }
        }
        self.current.project_manager.lock().initialize(folders);
        let settings = self
            .current
            .settings
            .rebind(self.current.project_manager.lock().workspace_folders());
        self.apply_settings(settings);
    }
}

impl Snapshot {
    pub fn base_module(&self) -> &BaseModule {
        &self.base_module
    }
    pub fn diagnostics(&self) -> &DiagnosticsEngine {
        &self.diagnostics
    }

    /// Only captured editor buffers qualify as request roots, never loader entries.
    pub fn document(&self, uri: &str) -> Option<Arc<VirtualFile>> {
        let url = pkl_vfs::effective_uri(&fluent_uri::Uri::parse(uri.to_owned()).ok()?).ok()?;
        self.buffers.get(&url).cloned()
    }

    pub fn open_document_analysis(&self, uri: &str) -> Result<Option<AnalysisSnapshot>, FileError> {
        self.document(uri)
            .map(|file| {
                self.parse_module(file)
                    .and_then(|module| self.prepare_analysis(module))
            })
            .transpose()
    }

    fn project_for(&self, source: &VirtualFile) -> Option<Arc<PklProject>> {
        let path = url_to_path(&source.url).ok()?;
        self.project_manager.lock().get_pkl_project_shared(&path)
    }

    /// Resolve a single URI owner; globs have a separate multi-target path.
    pub fn resolve_import_file(
        &self,
        tree: &SyntaxTree,
        import: NodeId,
        source: &VirtualFile,
    ) -> Option<Arc<VirtualFile>> {
        if !tree.kind(import).is_module_uri_owner() || tree.is_glob_import(import) {
            return None;
        }
        let literal = tree.child(import, NodeKind::is_string_literal)?;
        let uri = tree.escaped_string_literal(literal)?;
        let settings = &self.settings;
        let cache_dir = settings.effective_cache_dir();
        let project = self.project_for(source);
        ImportResolver::with_settings(&self.file_manager, &settings.module_paths, cache_dir)
            .resolve_file(&uri, source, project.as_deref())
    }

    pub fn resolve_import_module(
        &self,
        tree: &SyntaxTree,
        import: NodeId,
        source: &VirtualFile,
    ) -> Result<Option<ParsedModule>, FileError> {
        self.resolve_import_file(tree, import, source)
            .map(|file| self.parse_module(file))
            .transpose()
    }

    pub(crate) fn file_id(&self, file: &VirtualFile) -> FileId {
        if file.url.as_str() == "pkl:base" {
            FileId::BASE
        } else {
            self.file_registry.lock().register(&file.url)
        }
    }

    /// `pkl:base` shares the type system's reserved identity and exact syntax tree.
    pub fn parse_module(&self, file: Arc<VirtualFile>) -> Result<ParsedModule, FileError> {
        let file_id = self.file_id(&file);
        let tree = if file_id == FileId::BASE {
            Arc::clone(self.base_module().tree())
        } else {
            file.parsed_tree()?
        };
        Ok(ParsedModule {
            file_id,
            file,
            tree,
        })
    }

    /// Include declarations, import expressions, and extends/amends clauses.
    /// Package/remote descendants retain the importing project's dependency context.
    /// Local files select their own nearest project. No project lock spans I/O.
    pub fn prepare_analysis(&self, module: ParsedModule) -> Result<AnalysisSnapshot, FileError> {
        let settings = &self.settings;
        let cache_dir = settings.effective_cache_dir();
        let resolver =
            ImportResolver::with_settings(&self.file_manager, &settings.module_paths, cache_dir);
        let mut trees = HashMap::new();
        let mut imports = HashMap::new();
        trees.insert(module.file_id, Arc::clone(&module.tree));
        let context = self.project_for(&module.file);
        let mut pending = vec![(module.file_id, Arc::clone(&module.file), context)];
        while let Some((file_id, source, inherited_context)) = pending.pop() {
            let context = if source.url.scheme().as_str() == "file" {
                self.project_for(&source)
            } else {
                inherited_context
            };
            let tree = Arc::clone(&trees[&file_id]);
            let mut nodes = vec![tree.root()];
            while let Some(node) = nodes.pop() {
                nodes.extend(tree.children(node).iter().rev().copied());
                if !tree.kind(node).is_module_uri_owner() {
                    continue;
                }
                let Some(literal) = tree.child(node, NodeKind::is_string_literal) else {
                    continue;
                };
                let Some(uri) = tree.escaped_string_literal(literal) else {
                    continue;
                };
                let key = NodeRef::new(file_id, node);
                if tree.is_glob_import(node) {
                    let mut targets = Vec::new();
                    for file in resolver.resolve_glob(&uri, &source, context.as_deref()) {
                        targets.push(self.pin_target(file, &context, &mut trees, &mut pending)?);
                    }
                    let resolution = if targets.is_empty() {
                        resolver
                            .missing_package_uri(&uri, &source, context.as_deref())
                            .map(|uri| ImportResolution::Unresolved {
                                package_to_download: Some(uri),
                            })
                            .unwrap_or(ImportResolution::Glob(targets))
                    } else {
                        ImportResolution::Glob(targets)
                    };
                    imports.insert(key, resolution);
                } else if let Some(file) = resolver.resolve_file(&uri, &source, context.as_deref())
                {
                    let target = self.pin_target(file, &context, &mut trees, &mut pending)?;
                    imports.insert(key, ImportResolution::Single(target));
                } else {
                    imports.insert(
                        key,
                        ImportResolution::Unresolved {
                            package_to_download: resolver.missing_package_uri(
                                &uri,
                                &source,
                                context.as_deref(),
                            ),
                        },
                    );
                }
            }
        }
        Ok(AnalysisSnapshot {
            module,
            trees,
            imports,
        })
    }

    fn pin_target(
        &self,
        file: Arc<VirtualFile>,
        context: &Option<Arc<PklProject>>,
        trees: &mut HashMap<FileId, Arc<SyntaxTree>>,
        pending: &mut Vec<PendingModule>,
    ) -> Result<NodeRef, FileError> {
        let file_id = self.file_id(&file);
        if let Entry::Vacant(entry) = trees.entry(file_id) {
            let tree = if file_id == FileId::BASE {
                Arc::clone(self.base_module().tree())
            } else {
                file.parsed_tree()?
            };
            entry.insert(tree);
            pending.push((file_id, file, context.clone()));
        }
        Ok(NodeRef::new(file_id, NodeId::ROOT))
    }
}
