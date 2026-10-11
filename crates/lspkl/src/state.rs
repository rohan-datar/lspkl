//! Process-wide state shared across LSP handlers.

use std::collections::{HashMap, hash_map::Entry};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, LazyLock};

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

pub struct ServerState {
    pub file_manager: FileManager,
    pub file_registry: parking_lot::Mutex<FileRegistry>,
    base_module: LazyLock<BaseModule>,
    diagnostics: LazyLock<DiagnosticsEngine>,
    pub project_manager: parking_lot::Mutex<pkl_project::project_manager::ProjectManager>,
    pub package_manager: parking_lot::Mutex<pkl_project::package_manager::PackageManager>,
    /// Default package cache; live workspace settings may override it.
    pub package_cache_dir: Option<PathBuf>,
    pub settings: parking_lot::RwLock<Arc<WorkspaceSettings>>,
    pub client_configuration: AtomicBool,
    pub(crate) open_documents:
        parking_lot::Mutex<HashMap<FileId, Arc<tower_lsp_server::lsp_types::Uri>>>,
    module_cache_dir: PathBuf,
}

impl Default for ServerState {
    fn default() -> Self {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
        let package_cache_dir = home.map(|dir| PathBuf::from(dir).join(".pkl/cache"));
        let file_manager = FileManager::default();
        if let Some(cache_dir) = &package_cache_dir {
            file_manager.set_cache_dir(cache_dir.clone());
        }
        let module_cache_dir = file_manager.cache_dir();
        Self {
            file_manager,
            file_registry: parking_lot::Mutex::new(FileRegistry::default()),
            base_module: LazyLock::new(|| {
                BaseModule::load_vendored().expect("vendored base.pkl must load")
            }),
            diagnostics: LazyLock::new(DiagnosticsEngine::default),
            project_manager: parking_lot::Mutex::new(Default::default()),
            package_manager: parking_lot::Mutex::new(
                pkl_project::package_manager::PackageManager::new(Default::default()),
            ),
            package_cache_dir,
            settings: parking_lot::RwLock::new(Arc::new(WorkspaceSettings::default())),
            client_configuration: AtomicBool::new(false),
            open_documents: parking_lot::Mutex::new(HashMap::new()),
            module_cache_dir,
        }
    }
}

impl std::fmt::Debug for ServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerState")
            .field("file_manager", &self.file_manager)
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl ServerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn base_module(&self) -> &BaseModule {
        &self.base_module
    }

    pub fn diagnostics(&self) -> &DiagnosticsEngine {
        &self.diagnostics
    }

    pub fn apply_settings(&self, mut settings: WorkspaceSettings) {
        let cli = PklCli {
            binary_path: settings.cli.path.clone(),
        };
        {
            let mut projects = self.project_manager.lock();
            settings.resolve_paths(projects.workspace_folders());
            projects.configure(cli.clone(), settings.projects.excluded_directories.clone());
        }
        self.package_manager.lock().set_cli(cli);
        let cache_dir = settings
            .resolved_cache_dir
            .as_ref()
            .or(self.package_cache_dir.as_ref())
            .unwrap_or(&self.module_cache_dir);
        self.file_manager.set_cache_dir(cache_dir.clone());
        *self.settings.write() = Arc::new(settings);
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
        let settings = Arc::clone(&self.settings.read());
        let cache_dir = settings
            .resolved_cache_dir
            .as_ref()
            .or(self.package_cache_dir.as_ref())
            .unwrap_or(&self.module_cache_dir);
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
            self.file_manager.parse(&file.url)?
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
        let settings = Arc::clone(&self.settings.read());
        let cache_dir = settings
            .resolved_cache_dir
            .as_ref()
            .or(self.package_cache_dir.as_ref())
            .unwrap_or(&self.module_cache_dir);
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
                self.file_manager.parse(&file.url)?
            };
            entry.insert(tree);
            pending.push((file_id, file, context.clone()));
        }
        Ok(NodeRef::new(file_id, NodeId::ROOT))
    }
}
