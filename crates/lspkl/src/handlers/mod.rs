//! Synchronous protocol handlers. Callers run compute against one pinned snapshot.

pub mod commands;
pub mod diagnostics;
pub mod documents;
pub mod lifecycle;

#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    #[error("{0}")]
    InvalidParams(String),
    #[error("{0}")]
    Domain(String),
}
impl From<pkl_vfs::FileError> for HandlerError {
    fn from(error: pkl_vfs::FileError) -> Self {
        Self::Domain(error.to_string())
    }
}
pub type Result<T> = std::result::Result<T, HandlerError>;
