//! Package cache lookups and downloads.

use serde_json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use fluent_uri::Uri;

use crate::cli::PklCli;
use crate::dto::{
    Dependency, PackageDependency, PackageMetadata, PackageUri, PklProject, resolved_to_dependency,
};

#[derive(Debug, Clone)]
pub struct PackageLibraryRoots {
    pub metadata_file: PathBuf,
    pub zip_file: PathBuf,
    pub package_root: Uri<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error(transparent)]
    Cli(#[from] crate::cli::PklCliError),
    #[error("failed to read package metadata: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot represent ZIP path as file URI: {0}")]
    InvalidZipPath(PathBuf),
    #[error("invalid jar URI: {0}")]
    InvalidJarUri(#[from] fluent_uri::ParseError),
    #[error("invalid package cache path: {0}")]
    InvalidCachePath(PathBuf),
    #[error("package metadata checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("downloaded package metadata is absent from the cache")]
    MissingMetadata,
}

#[allow(dead_code)]
pub struct PackageManager {
    cli: PklCli,
}

impl PackageManager {
    pub fn new(cli: PklCli) -> Self {
        Self { cli }
    }
    pub fn set_cli(&mut self, cli: PklCli) {
        self.cli = cli;
    }

    /// Locate the cached metadata and ZIP, and form its `jar:` root URI.
    pub fn get_library_roots(
        &self,
        dependency: &PackageDependency,
        cache_dir: &Path,
    ) -> Result<Option<PackageLibraryRoots>, PackageError> {
        let metadata_candidates = dependency.uri.relative_metadata_files();
        let zip_candidates = dependency.uri.relative_zip_files();
        for relative in metadata_candidates.iter().chain(&zip_candidates) {
            if Path::new(relative)
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
            {
                return Err(PackageError::InvalidCachePath(PathBuf::from(relative)));
            }
        }
        let Some(metadata_file) = metadata_candidates.iter().find_map(|rel_path| {
            let file_path = cache_dir.join(rel_path);
            if !file_path.exists() {
                return None;
            }
            Some(file_path)
        }) else {
            return Ok(None);
        };

        let Some(zip_file) = zip_candidates.iter().find_map(|rel_path| {
            let file_path = cache_dir.join(rel_path);
            if !file_path.exists() {
                return None;
            }
            Some(file_path)
        }) else {
            return Ok(None);
        };
        if let Some(checksum) = dependency
            .uri
            .checksums
            .as_ref()
            .or(dependency.checksums.as_ref())
        {
            verify_checksum(&metadata_file, checksum)?;
        }

        let zip_url = pkl_vfs::path_to_uri(&zip_file.canonicalize()?)
            .map_err(|_| PackageError::InvalidZipPath(zip_file.clone()))?;
        let package_root = Uri::parse(format!("jar:{zip_url}!/"))
            .map_err(|(err, _)| PackageError::InvalidJarUri(err))?;

        Ok(Some(PackageLibraryRoots {
            metadata_file,
            zip_file,
            package_root,
        }))
    }

    /// Named dependencies from a cached package's metadata.
    pub fn get_resolved_dependencies(
        &self,
        dependency: &PackageDependency,
        owner_project: Option<&PklProject>,
        context: Option<&PklProject>,
        cache_dir: &Path,
    ) -> Result<Option<BTreeMap<String, Dependency>>, PackageError> {
        let Some(roots) = self.get_library_roots(dependency, cache_dir)? else {
            return Ok(None);
        };
        let metadata_file = fs::read_to_string(roots.metadata_file)?;
        let metadata = serde_json::from_str::<PackageMetadata>(&metadata_file)?;

        let mut named_map = BTreeMap::new();
        if let Some(owner) = owner_project {
            let Some(deps) = owner.project_deps.as_ref() else {
                return Ok(None);
            };
            let Some(project_dir) = owner.project_dir() else {
                return Ok(None);
            };
            // Project-owned packages only expose dependencies present in the
            // owning project's resolved dependency graph.
            for (name, dep) in metadata.dependencies {
                let Some(resolved) = deps.get_resolved_dependency(&dep.uri.to_display_string())
                else {
                    continue;
                };
                if let Some(resolved_dep) = resolved_to_dependency(resolved, &project_dir) {
                    named_map.insert(name, resolved_dep);
                }
            }
        } else {
            for (name, dep) in metadata.dependencies {
                let from_context = context.and_then(|project| {
                    let project_dir = project.project_dir()?;
                    let resolved = project
                        .project_deps
                        .as_ref()?
                        .get_resolved_dependency(&dep.uri.to_display_string())?;
                    resolved_to_dependency(resolved, &project_dir)
                });
                let resolved_dep = from_context.unwrap_or_else(|| {
                    Dependency::Package(PackageDependency {
                        uri: dep.uri,
                        checksums: dep.checksums,
                    })
                });
                named_map.insert(name, resolved_dep);
            }
        }
        Ok(Some(named_map))
    }

    /// Download + extract `uri` into the module cache. Blocking; shell out via
    /// [`crate::cli::PklCli::download_package`].
    pub fn download_package(&self, uri: &PackageUri, cache_dir: &Path) -> Result<(), PackageError> {
        let mut request = uri.to_display_string();
        if let Some(checksum) = &uri.checksums {
            request.push_str("::sha256:");
            request.push_str(&checksum.sha256);
        }
        self.cli.download_package(&request, cache_dir)?;
        if let Some(checksum) = &uri.checksums {
            let metadata = uri
                .relative_metadata_files()
                .into_iter()
                .map(|relative| cache_dir.join(relative))
                .find(|path| path.is_file())
                .ok_or(PackageError::MissingMetadata)?;
            verify_checksum(&metadata, checksum)?;
        }
        Ok(())
    }
}

fn verify_checksum(path: &Path, checksum: &crate::dto::Checksums) -> Result<(), PackageError> {
    use sha2::{Digest, Sha256};
    let actual = format!("{:x}", Sha256::digest(fs::read(path)?));
    if !actual.eq_ignore_ascii_case(&checksum.sha256) {
        return Err(PackageError::ChecksumMismatch {
            expected: checksum.sha256.clone(),
            actual,
        });
    }
    Ok(())
}
