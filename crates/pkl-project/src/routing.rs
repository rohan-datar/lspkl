//! Owned routing reads, separated from incremental analysis.
use crate::{PackageDependency, cli::PklCli, package_manager::PackageManager};
use fluent_uri::Uri;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum RoutingRequest {
    ModuleRoot(PathBuf),
    Package {
        dependency: PackageDependency,
        cache_dir: PathBuf,
    },
}
impl RoutingRequest {
    /// Blocking filesystem operation; never call from a database query.
    pub fn load(&self) -> Option<Uri<String>> {
        match self {
            Self::ModuleRoot(path) => crate::modulepath::root_uri(path),
            Self::Package {
                dependency,
                cache_dir,
            } => PackageManager::new(PklCli::default())
                .get_library_roots(dependency, cache_dir)
                .ok()
                .flatten()
                .map(|roots| roots.package_root),
        }
    }
}

pub trait RoutingFacts {
    fn root(&self, request: RoutingRequest) -> Option<Uri<String>>;
}
pub struct DiskRoutingFacts;
impl RoutingFacts for DiskRoutingFacts {
    fn root(&self, request: RoutingRequest) -> Option<Uri<String>> {
        request.load()
    }
}
pub fn package_request(dependency: &PackageDependency, cache_dir: &Path) -> RoutingRequest {
    RoutingRequest::Package {
        dependency: dependency.clone(),
        cache_dir: cache_dir.to_owned(),
    }
}
