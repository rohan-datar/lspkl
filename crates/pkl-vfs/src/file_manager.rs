//! Concurrent registry of files the language server knows about.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use dashmap::DashMap;
use fluent_uri::Uri;
use pkl_syntax::ast::SyntaxTree;
use thiserror::Error;
use url;

use crate::virtual_file::{Origin, VirtualFile};

#[derive(Debug, Error)]
pub enum FileError {
    #[error("URL scheme `{0}` is not supported")]
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
    Parse(#[from] pkl_syntax::ParseError),

    #[error("file is not open: {0}")]
    NotOpen(Uri<String>),

    #[error("Path `{0}` could not be converted to a URL")]
    InvalidUri(std::path::PathBuf),

    #[error("unable to parse uri: {0}")]
    UriParse(#[from] fluent_uri::ParseError),
    #[error("invalid source URI or contents: {0}")]
    InvalidSource(String),
    #[error("archive error: {0}")]
    Archive(#[from] zip::result::ZipError),
    #[error("HTTPS error: {0}")]
    Https(#[from] reqwest::Error),
}

/// Owns every `VirtualFile` the server is currently tracking.
#[derive(Debug)]
pub struct FileManager {
    files: Arc<DashMap<Uri<String>, Arc<VirtualFile>>>,
    open_documents: Option<Arc<std::collections::HashMap<Uri<String>, Arc<VirtualFile>>>>,
    cache_dir: RwLock<PathBuf>,
    client: RwLock<Option<reqwest::blocking::Client>>,
}

impl Default for FileManager {
    fn default() -> Self {
        Self::new()
    }
}

impl From<(fluent_uri::ParseError, String)> for FileError {
    fn from((error, _): (fluent_uri::ParseError, String)) -> Self {
        Self::UriParse(error)
    }
}

impl FileManager {
    /// Pin editor buffers while sharing the loader cache. Construct on workspace
    /// writes, so capturing a request only clones the resulting Arc.
    pub fn with_open_documents(
        &self,
        open_documents: Arc<std::collections::HashMap<Uri<String>, Arc<VirtualFile>>>,
        cache_dir: PathBuf,
    ) -> Self {
        Self {
            files: Arc::clone(&self.files),
            open_documents: Some(open_documents),
            cache_dir: RwLock::new(cache_dir),
            client: RwLock::new(self.client.read().unwrap().clone()),
        }
    }
    pub fn new() -> Self {
        Self {
            files: Arc::new(DashMap::new()),
            open_documents: None,
            cache_dir: RwLock::new(crate::sources::default_cache()),
            client: RwLock::new(None),
        }
    }

    /// Number of files currently tracked. Useful in tests.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Get a known file without disk or network access.
    pub fn get(&self, url: &Uri<String>) -> Option<Arc<VirtualFile>> {
        let url = crate::effective_uri(url).ok()?;
        if let Some(documents) = &self.open_documents
            && let Some(file) = documents.get(&url)
        {
            return Some(Arc::clone(file));
        }
        self.files.get(&url).map(|f| f.value().clone())
    }

    /// Register a file the editor just opened. Implements the `didOpen` flow.
    pub fn open(
        &self,
        url: Uri<String>,
        contents: String,
        version: Option<i32>,
    ) -> Arc<VirtualFile> {
        let url = crate::effective_uri(&url).unwrap_or(url);
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
        let url = crate::effective_uri(url)?;
        let Some(file) = self.files.get(&url) else {
            return Err(FileError::NotOpen(url));
        };
        file.set_contents(new_contents, new_version);
        Ok(file.clone())
    }

    /// Evict mutable backing files on close; immutable SDK sources stay cached.
    pub fn close(&self, url: &Uri<String>) {
        if let Ok(url) = crate::effective_uri(url) {
            self.files.remove_if(&url, |_, file| !file.is_read_only());
        }
    }

    /// Load a source synchronously. Async callers must use a blocking worker.
    pub fn get_or_load(&self, url: &Uri<String>) -> Result<Arc<VirtualFile>, FileError> {
        let url = crate::effective_uri(url)?;
        if let Some(file) = self.get(&url) {
            return Ok(file);
        }
        self.load_source(url)
    }

    /// Load backing source without the captured editor overlay.
    pub fn get_or_load_source(&self, url: &Uri<String>) -> Result<Arc<VirtualFile>, FileError> {
        let url = crate::effective_uri(url)?;
        if let Some(file) = self.files.get(&url) {
            return Ok(Arc::clone(file.value()));
        }
        self.load_source(url)
    }

    fn load_source(&self, url: Uri<String>) -> Result<Arc<VirtualFile>, FileError> {
        if url.scheme().as_str() == "pkl" {
            let contents = crate::stdlib::source(url.path().as_str())
                .ok_or_else(|| FileError::InvalidSource(url.to_string()))?;
            let file = Arc::new(VirtualFile::from_embedded_source(url.clone(), contents));
            return Ok(self.files.entry(url).or_insert(file).clone());
        }
        let contents = match url.scheme().as_str() {
            "jar" => crate::sources::archive_contents(&url)?,
            "https" => {
                let client = self.http_client()?;
                crate::sources::https_contents(&url, &client, &self.cache_dir.read().unwrap())?
            }
            "file" => {
                let path = url_to_path(&url)?;
                std::fs::read_to_string(&path).map_err(|source| FileError::Io { path, source })?
            }
            scheme => return Err(FileError::UnsupportedScheme(scheme.to_owned())),
        };
        // Retain an already cached source if another loader won the race.
        let file = Arc::new(VirtualFile::new(
            url.clone(),
            Self::origin_for(&url),
            contents,
            None,
        ));
        Ok(self.files.entry(url).or_insert(file).clone())
    }

    pub fn set_cache_dir(&self, path: PathBuf) {
        *self.cache_dir.write().unwrap() = path;
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.cache_dir.read().unwrap().clone()
    }

    /// Add trusted roots without disabling certificate or hostname verification.
    pub fn with_trusted_certificates(
        certificates: Vec<reqwest::Certificate>,
        timeout: std::time::Duration,
    ) -> Result<Self, FileError> {
        let mut builder = reqwest::blocking::Client::builder()
            .https_only(true)
            .timeout(timeout);
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
        let manager = Self::new();
        *manager.client.write().unwrap() = Some(builder.build()?);
        Ok(manager)
    }

    fn http_client(&self) -> Result<reqwest::blocking::Client, FileError> {
        let mut client = self.client.write().unwrap();
        if let Some(client) = client.as_ref() {
            return Ok(client.clone());
        }
        let built = reqwest::blocking::Client::builder()
            .https_only(true)
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        *client = Some(built.clone());
        Ok(built)
    }

    pub fn resolve_uri(
        &self,
        source: &Uri<String>,
        reference: &str,
    ) -> Result<Uri<String>, FileError> {
        crate::uri::resolve(source, reference)
    }

    pub fn list_files(&self, root: &Uri<String>) -> Result<Vec<Uri<String>>, FileError> {
        crate::sources::list(&crate::effective_uri(root)?)
    }

    /// Fetch a file by path, normalizing URI dot segments before registry lookup.
    pub fn get_modulepath_file(&self, path: &Path) -> Result<Arc<VirtualFile>, FileError> {
        let uri = path_to_uri(path)?.normalize();
        self.get_or_load(&uri)
    }

    /// Parse a file (cached). Returns the cached tree if one is present;
    /// otherwise parses, caches, and returns it.
    ///
    /// Tree-sitter parsing is synchronous and CPU-bound. Callers in async
    /// contexts should wrap this in `tokio::task::spawn_blocking` — the
    /// FileManager itself stays sync to keep tests easy.
    pub fn parse(&self, url: &Uri<String>) -> Result<Arc<SyntaxTree>, FileError> {
        let url = crate::effective_uri(url)?;
        let Some(file) = self.get(&url) else {
            return Err(FileError::NotOpen(url));
        };

        file.parsed_tree().map_err(FileError::Parse)
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

/// Convert filesystem paths to RFC-compliant file URIs, including literal brackets.
pub fn path_to_uri(path: &Path) -> Result<Uri<String>, FileError> {
    let url = url::Url::from_file_path(path).map_err(|_| FileError::InvalidUri(path.to_owned()))?;
    let value: String = url.into();
    if !value.contains(['[', ']']) {
        return Ok(Uri::parse(value)?);
    }
    let mut encoded = String::with_capacity(value.len() + 4);
    for ch in value.chars() {
        match ch {
            '[' => encoded.push_str("%5B"),
            ']' => encoded.push_str("%5D"),
            _ => encoded.push(ch),
        }
    }
    Ok(Uri::parse(encoded)?)
}

/// Helper for tests and `get_or_load`: convert a `file://` URL into a path.
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
