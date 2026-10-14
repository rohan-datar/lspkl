//! Project and package management

pub mod cli;
pub mod dto;
pub mod import_resolver;
pub mod module_resolver;
pub mod modulepath;
pub use module_resolver::ModuleResolver;
pub mod package_manager;
pub mod project_manager;

pub use dto::{
    Checksums, Dependency, DerivedProjectMetadata, EvaluatorSettings, LocalDependency,
    LocalProjectDependency, PackageDependency, PackageUri, PklProject, PklWorkspaceState,
    PklWorkspaceStateEntry, ProjectDeps, RemoteDependency, ResolvedDependency,
};
