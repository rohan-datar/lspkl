//! Process-wide state shared across LSP handlers.

use pkl_vfs::FileManager;

/// Shared state across all LSP request handlers. Wrap in `Arc` to hand to
/// async tasks.
///
/// More fields will land here in later phases:
/// - `base_module: OnceCell<BaseModule>`
/// - `diagnostics: DiagnosticsEngine`
/// - `project_manager: ProjectManager`
/// - `package_manager: PackageManager`
/// - `event_tx: tokio::sync::broadcast::Sender<Event>` (when needed)
#[derive(Debug, Default)]
pub struct ServerState {
    pub file_manager: FileManager,
}

impl ServerState {
    pub fn new() -> Self {
        Self::default()
    }
}
