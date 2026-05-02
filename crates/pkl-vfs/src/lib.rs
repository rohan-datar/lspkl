//! Virtual file system for lspkl.

pub mod file_manager;
pub mod virtual_file;

pub use file_manager::{FileError, FileManager};
pub use virtual_file::{Origin, VirtualFile};
