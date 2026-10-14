//! Source loading and URI routing for lspkl.
mod source_loader;
mod sources;
pub mod stdlib;
mod uri;
use fluent_uri::Uri;
pub use source_loader::{Source, SourceLoader, StandardSourceLoader};
pub use sources::default_cache;
use std::path::Path;
use thiserror::Error;
pub use uri::{archive_file_uri, effective_uri, parse_source_uri, to_editor_uri};
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

impl From<(fluent_uri::ParseError, String)> for FileError {
    fn from((error, _): (fluent_uri::ParseError, String)) -> Self {
        Self::UriParse(error)
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

/// Convert a `file://` URL into a path.
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
