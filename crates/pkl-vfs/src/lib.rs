//! Virtual file system for lspkl.

pub mod file_manager;
mod sources;
pub mod stdlib;
mod uri;
pub mod virtual_file;
pub use uri::{effective_uri, to_editor_uri};

pub use file_manager::{FileError, FileManager, path_to_uri, url_to_path};
pub use virtual_file::{Origin, VirtualFile};
