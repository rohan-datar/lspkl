use crate::{FileError, effective_uri, path_to_uri, url_to_path};
use fluent_uri::Uri;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

/// Immutable source bytes. Editor state and syntax trees belong to the database.
#[derive(Debug)]
pub struct Source {
    pub url: Uri<String>,
    pub text: Arc<str>,
}
impl Source {
    pub fn contents(&self) -> &str {
        &self.text
    }
}

/// Synchronous, network-free source access, including database-backed overlays.
pub trait SourceLoader {
    fn load(&self, uri: &Uri<String>, cache_dir: &Path) -> Result<Arc<Source>, FileError>;
    fn list_files(&self, root: &Uri<String>) -> Result<Vec<Uri<String>>, FileError>;
    fn resolve_uri(&self, source: &Uri<String>, reference: &str) -> Result<Uri<String>, FileError> {
        crate::uri::resolve(source, reference)
    }
    fn get_modulepath_file(&self, path: &Path, cache_dir: &Path) -> Result<Arc<Source>, FileError> {
        self.load(&path_to_uri(path)?.normalize(), cache_dir)
    }
}

#[derive(Debug, Default)]
pub struct StandardSourceLoader {
    client: Mutex<Option<reqwest::blocking::Client>>,
}
impl StandardSourceLoader {
    pub fn new() -> Self {
        Self::default()
    }
    /// Trust additional roots while retaining certificate and hostname verification.
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
        Ok(Self {
            client: Mutex::new(Some(builder.build()?)),
        })
    }
    /// Explicit network operation. Queries use only the resulting disk cache.
    pub fn fetch(&self, uri: &Uri<String>, cache_dir: &Path) -> Result<Arc<Source>, FileError> {
        let url = effective_uri(uri)?;
        if url.scheme().as_str() != "https" {
            return Err(FileError::UnsupportedScheme(
                url.scheme().as_str().to_owned(),
            ));
        }
        let client = {
            let mut client = self.client.lock().unwrap();
            if client.is_none() {
                *client = Some(
                    reqwest::blocking::Client::builder()
                        .https_only(true)
                        .timeout(std::time::Duration::from_secs(30))
                        .build()?,
                );
            }
            client.as_ref().unwrap().clone()
        };
        let text = crate::sources::https_contents(&url, &client, cache_dir)?.into();
        Ok(Arc::new(Source { url, text }))
    }
}
impl SourceLoader for StandardSourceLoader {
    fn load(&self, uri: &Uri<String>, cache_dir: &Path) -> Result<Arc<Source>, FileError> {
        let url = effective_uri(uri)?;
        let text: Arc<str> = match url.scheme().as_str() {
            "pkl" => crate::stdlib::source(url.path().as_str())
                .ok_or_else(|| FileError::InvalidSource(url.to_string()))?
                .into(),
            "jar" => crate::sources::archive_contents(&url)?.into(),
            "https" => {
                let path = crate::sources::https_cache_path(&url, cache_dir)?;
                std::fs::read_to_string(&path)
                    .map_err(|e| crate::sources::io(&path, e))?
                    .into()
            }
            "file" => {
                let path = url_to_path(&url)?;
                std::fs::read_to_string(&path)
                    .map_err(|e| crate::sources::io(&path, e))?
                    .into()
            }
            scheme => return Err(FileError::UnsupportedScheme(scheme.to_owned())),
        };
        Ok(Arc::new(Source { url, text }))
    }
    fn list_files(&self, root: &Uri<String>) -> Result<Vec<Uri<String>>, FileError> {
        crate::sources::list(&effective_uri(root)?)
    }
}
