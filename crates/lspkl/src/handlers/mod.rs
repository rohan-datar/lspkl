//! Synchronous protocol handlers. Callers run compute against one pinned snapshot.

pub mod commands;
pub mod diagnostics;
pub mod documents;
pub mod lifecycle;

#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    #[error("document changed during analysis")]
    ContentModified,
    #[error("{0}")]
    InvalidParams(String),
    #[error("{0}")]
    Domain(String),
}
impl From<lspkl_vfs::FileError> for HandlerError {
    fn from(error: lspkl_vfs::FileError) -> Self {
        Self::Domain(error.to_string())
    }
}
impl From<crate::state::WorkspaceError> for HandlerError {
    fn from(error: crate::state::WorkspaceError) -> Self {
        Self::Domain(error.to_string())
    }
}
pub type Result<T> = std::result::Result<T, HandlerError>;
