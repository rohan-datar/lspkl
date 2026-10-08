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
    pub package_root: Uri<String>, // TODO: This is a jar: file representing the package root. This will change once jar file handling is implemented in pkl-vfs
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
}

#[allow(dead_code)]
pub struct PackageManager {
    cli: PklCli,
}

impl PackageManager {
    pub fn new(cli: PklCli) -> Self {
        Self { cli }
    }

    /// Locate the cached metadata and ZIP, and form its `jar:` root URI.
    pub fn get_library_roots(
        &self,
        dependency: &PackageDependency,
        cache_dir: &Path,
    ) -> Result<Option<PackageLibraryRoots>, PackageError> {
        let Some(metadata_file) =
            dependency
                .uri
                .relative_metadata_files()
                .iter()
                .find_map(|rel_path| {
                    let file_path = cache_dir.join(rel_path);
                    if !file_path.exists() {
                        return None;
                    }
                    Some(file_path)
                })
        else {
            return Ok(None);
        };

        let Some(zip_file) = dependency
            .uri
            .relative_zip_files()
            .iter()
            .find_map(|rel_path| {
                let file_path = cache_dir.join(rel_path);
                if !file_path.exists() {
                    return None;
                }
                Some(file_path)
            })
        else {
            return Ok(None);
        };

        let zip_url = url::Url::from_file_path(zip_file.canonicalize()?)
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
        self.cli
            .download_package(&uri.to_display_string(), cache_dir)?;
        Ok(())
    }
}
