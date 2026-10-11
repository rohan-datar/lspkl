//! lspkl: tower-lsp-server-based language server for Pkl.

pub mod file_registry;
pub mod module_uri_completion;
pub mod server;
pub mod settings;
pub mod state;

pub use server::PklBackend;
pub use state::ServerState;
