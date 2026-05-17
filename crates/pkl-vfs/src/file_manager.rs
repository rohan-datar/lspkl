//! Concurrent registry of files the language server knows about.
//!
//! Mirrors `VirtualFileManager.kt` in pkl-lsp.

use std::fs;
use std::sync::Arc;

use dashmap::DashMap;
use fluent_uri::Uri;
use pkl_analysis::ast::SyntaxTree;
use pkl_analysis::parse;
use thiserror::Error;
use url;

use crate::virtual_file::{Origin, VirtualFile};

#[derive(Debug, Error)]
pub enum FileError {
    #[error("URL scheme `{0}` is not supported in this phase")]
    UnsupportedScheme(String),

    #[error("URL `{0}` could not be converted to a filesystem path")]
    InvalidPath(Uri<String>),

    #[error("I/O error reading {path:?}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("parse error: {0}")]
    Parse(#[from] pkl_analysis::ParseError),

    #[error("file is not open: {0}")]
    NotOpen(Uri<String>),
}

/// Owns every `VirtualFile` the server is currently tracking.
#[derive(Debug, Default)]
pub struct FileManager {
    files: DashMap<Uri<String>, Arc<VirtualFile>>,
}

impl FileManager {
    pub fn new() -> Self {
        Self {
            files: DashMap::new(),
        }
    }

    /// Number of files currently tracked. Useful in tests.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Get the file for `url` if it has been opened or fetched. Does NOT read
    /// from disk; use [`get_or_load`] for that.
    pub fn get(&self, url: &Uri<String>) -> Option<Arc<VirtualFile>> {
        self.files.get(url).map(|f| f.value().clone())
    }

    /// Register a file the editor just opened. Implements the `didOpen` flow.
    pub fn open(
        &self,
        url: Uri<String>,
        contents: String,
        version: Option<i32>,
    ) -> Arc<VirtualFile> {
        let origin = Self::origin_for(&url);
        let file = Arc::new(VirtualFile::new(url.clone(), origin, contents, version));
        self.files.insert(url.clone(), file.clone());
        file
    }

    /// Replace the contents for an already-open file. Implements the
    /// `didChange` flow with `TextDocumentSyncKind::Full`.
    ///
    /// Returns `Err` if the file is not currently open.
    pub fn update(
        &self,
        url: &Uri<String>,
        new_contents: String,
        new_version: Option<i32>,
    ) -> Result<Arc<VirtualFile>, FileError> {
        let Some(file) = self.files.get(url) else {
            return Err(FileError::NotOpen(url.clone()));
        };
        file.set_contents(new_contents, new_version);
        Ok(file.clone())
    }

    /// Remove a file from the registry. Implements the `didClose` flow.
    /// No-op if the file isn't tracked.
    pub fn close(&self, url: &Uri<String>) {
        self.files.remove(url);
    }

    /// Read a file from disk if it's not already in the registry. Used when
    /// resolving imports to files the editor never opened.
    pub fn get_or_load(&self, url: &Uri<String>) -> Result<Arc<VirtualFile>, FileError> {
        if let Some(file) = self.files.get(url) {
            return Ok(file.clone());
        };
        let path = url_to_path(url)?;
        match fs::read_to_string(path.clone()) {
            Ok(contents) => {
                let vf = self.open(url.clone(), contents, None);
                Ok(vf)
            }
            Err(err) => Err(FileError::Io { path, source: err }),
        }
    }

    /// Parse a file (cached). Returns the cached tree if one is present;
    /// otherwise parses, caches, and returns it.
    ///
    /// Tree-sitter parsing is synchronous and CPU-bound. Callers in async
    /// contexts should wrap this in `tokio::task::spawn_blocking` — the
    /// FileManager itself stays sync to keep tests easy.
    pub fn parse(&self, url: &Uri<String>) -> Result<Arc<SyntaxTree>, FileError> {
        let Some(file) = self.files.get(url) else {
            return Err(FileError::NotOpen(url.clone()));
        };

        match file.cached_tree() {
            Some(tree) => Ok(tree),
            None => match parse(file.contents().as_str()) {
                Ok(tree) => {
                    let tree_rc: Arc<SyntaxTree> = Arc::new(tree);
                    file.set_cached_tree(tree_rc.clone());
                    Ok(tree_rc)
                }
                Err(err) => Err(FileError::Parse(err)),
            },
        }
    }

    fn origin_for(url: &Uri<String>) -> Origin {
        match url.scheme().as_str() {
            "file" => Origin::Fs { url: url.clone() },
            "pkl" => Origin::Stdlib,
            "https" => Origin::Https,
            "jar" => Origin::Jar,
            _ => Origin::Ephemeral,
        }
    }
}

/// Helper for tests and `get_or_load`: convert a `file://` URL into a path.
/// Pulled out for unit testing.
pub fn url_to_path(url: &Uri<String>) -> Result<std::path::PathBuf, FileError> {
    let scheme = url.scheme().as_str();
    if scheme != "file" {
        return Err(FileError::UnsupportedScheme(scheme.into()));
    }
    // re-parse the url via the `url` crate for windows support
    let parsed = url::Url::parse(url.as_str()).map_err(|_| FileError::InvalidPath(url.clone()))?;
    parsed
        .to_file_path()
        .map_err(|_| FileError::InvalidPath(url.clone()))
}
