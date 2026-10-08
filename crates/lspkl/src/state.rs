//! Process-wide state shared across LSP handlers.

use std::path::PathBuf;
use std::sync::OnceLock;

use pkl_diagnostics::DiagnosticsEngine;
use pkl_types::BaseModule;
use pkl_vfs::FileManager;

/// Shared state across all LSP request handlers. Wrap in `Arc` to hand to
/// async tasks.
///
/// More fields will land here in later phases:
/// - `event_tx: tokio::sync::broadcast::Sender<Event>` (when needed)
pub struct ServerState {
    pub file_manager: FileManager,
    /// The `pkl:base` stdlib, parsed once on first use (see
    /// [`ServerState::base_module`]). Private so nothing can observe the
    /// uninitialized state.
    base_module: OnceLock<BaseModule>,
    /// The diagnostics pipeline (all 11 analyzers). Stateless, so one shared
    /// instance serves every request (see [`ServerState::diagnostics`]).
    diagnostics: OnceLock<DiagnosticsEngine>,
    /// Project discovery + dependency sync. Wrapped in a
    /// `parking_lot::Mutex` because `sync_projects` mutates discovered state.
    pub project_manager: parking_lot::Mutex<pkl_project::project_manager::ProjectManager>,
    /// Package download CLI; isolated from project sync so neither blocks the
    /// other's mutex. The handler runs its blocking CLI call off the runtime.
    pub package_manager: parking_lot::Mutex<pkl_project::package_manager::PackageManager>,
    /// Pkl's user cache, normally `<home>/.pkl/cache`. Tests can override it.
    pub package_cache_dir: Option<PathBuf>,
}

impl Default for ServerState {
    fn default() -> Self {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
        Self {
            file_manager: FileManager::default(),
            base_module: OnceLock::new(),
            diagnostics: OnceLock::new(),
            project_manager: parking_lot::Mutex::new(Default::default()),
            package_manager: parking_lot::Mutex::new(
                pkl_project::package_manager::PackageManager::new(Default::default()),
            ),
            package_cache_dir: home.map(|dir| PathBuf::from(dir).join(".pkl/cache")),
        }
    }
}

impl std::fmt::Debug for ServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `OnceLock` fields are opaque here; their contents are far too
        // large to be useful in a Debug dump anyway.
        f.debug_struct("ServerState")
            .field("file_manager", &self.file_manager)
            .field("base_module", &self.base_module.get().is_some())
            .field("diagnostics", &self.diagnostics.get().is_some())
            .field(
                "projects",
                &self
                    .project_manager
                    .try_lock()
                    .map(|pm| pm.projects().count()),
            )
            .finish()
    }
}

impl ServerState {
    pub fn new() -> Self {
        Self::default()
    }

    /// The stdlib module
    pub fn base_module(&self) -> &BaseModule {
        self.base_module
            .get_or_init(|| BaseModule::load_vendored().expect("vendored base.pkl must load"))
    }

    /// The diagnostics pipeline
    pub fn diagnostics(&self) -> &DiagnosticsEngine {
        self.diagnostics.get_or_init(DiagnosticsEngine::default)
    }
}
