//! lspkl: tower-lsp-server-based language server for Pkl.

pub mod handlers;
pub mod module_uri_completion;
pub(crate) mod perf;
pub mod server;
pub mod state;

pub use server::PklBackend;
pub use state::{Snapshot, Workspace};
