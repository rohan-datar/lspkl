//! Process-wide state shared across LSP handlers.

use std::sync::OnceLock;

use pkl_diagnostics::DiagnosticsEngine;
use pkl_types::BaseModule;
use pkl_vfs::FileManager;

/// Shared state across all LSP request handlers. Wrap in `Arc` to hand to
/// async tasks.
///
/// More fields will land here in later phases:
/// - `project_manager: ProjectManager`
/// - `package_manager: PackageManager`
/// - `event_tx: tokio::sync::broadcast::Sender<Event>` (when needed)
#[derive(Default)]
pub struct ServerState {
    pub file_manager: FileManager,
    /// The `pkl:base` stdlib, parsed once on first use (see
    /// [`ServerState::base_module`]). Private so nothing can observe the
    /// uninitialized state.
    base_module: OnceLock<BaseModule>,
    /// The diagnostics pipeline (all 11 analyzers). Stateless, so one shared
    /// instance serves every request (see [`ServerState::diagnostics`]).
    diagnostics: OnceLock<DiagnosticsEngine>,
}

impl std::fmt::Debug for ServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `OnceLock` fields are opaque here; their contents are far too
        // large to be useful in a Debug dump anyway.
        f.debug_struct("ServerState")
            .field("file_manager", &self.file_manager)
            .field("base_module", &self.base_module.get().is_some())
            .field("diagnostics", &self.diagnostics.get().is_some())
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
