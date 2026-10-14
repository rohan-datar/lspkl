//! Project discovery and dependency syncing.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cli::PklCli;
use crate::dto::{DerivedProjectMetadata, PklProject, PklWorkspaceState, ProjectDeps};

const DEPENDENCIES_EXPR: &str = "
      new JsonRenderer { omitNullProperties = false }
        .renderValue(new Dynamic {
          projectFileUri = module.projectFileUri
          packageUri = module.package?.uri
          declaredDependencies = module.dependencies.toMap().mapValues((_, value) ->
            if (value is RemoteDependency) value.uri
            else value.package.uri
          )
          evaluatorSettings = module.evaluatorSettings
        })";

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error(transparent)]
    Cli(#[from] crate::cli::PklCliError),
    #[error("failed to parse project metadata: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported project state schema version {0}")]
    UnsupportedStateVersion(i32),
    #[error("missing project path")]
    MissingPath,
    #[error("invalid project path in saved state: {0}")]
    InvalidStatePath(PathBuf),
}

/// Tracks every `.pkl-project` in the workspace and its resolution state.
#[derive(Clone)]
pub struct ProjectManager {
    cli: PklCli,
    /// Workspace roots as sent by the client (`initialize` / workspace
    /// folder changes).
    workspace_folders: Vec<PathBuf>,
    /// Discovered projects keyed by their `PklProject` file path.
    projects: BTreeMap<PathBuf, Arc<PklProject>>,
    excluded_directories: Vec<String>,
}

static SKIPPED_DIRS: &[&str] = &["node_modules", "target"];

/// Store state at `<workspace>/.lspkl/projects.json`.
/// Callers may also pass a state file directly inside the workspace.
fn state_workspace_root(state_file: &Path) -> &Path {
    let parent = state_file.parent().unwrap_or_else(|| Path::new("."));
    if parent.file_name() == Some(OsStr::new(".lspkl")) {
        parent.parent().unwrap_or(parent)
    } else {
        parent
    }
}

/// Recursively append `PklProject` files found below `directory`.
///
/// Do not follow directory symlinks. Skip hidden directories and common
/// dependency/VCS directories before descending into them.
fn walk_project_files(
    directory: &Path,
    project_files: &mut Vec<PathBuf>,
    excluded_directories: &[String],
) -> Result<(), std::io::Error> {
    for dirent in directory.read_dir()? {
        let dirent = dirent?;
        if dirent.file_type()?.is_file() {
            let name = dirent.file_name();
            if name == "PklProject" {
                project_files.push(dirent.path());
            }
            continue;
        }
        if dirent.file_type()?.is_symlink() {
            continue;
        }
        let dirname = dirent.file_name();
        if excluded_directories
            .iter()
            .any(|name| dirname == OsStr::new(name))
        {
            continue;
        }
        if dirname.to_string_lossy().starts_with('.') {
            continue;
        }
        walk_project_files(dirent.path().as_path(), project_files, excluded_directories)?;
    }
    Ok(())
}

impl Default for ProjectManager {
    fn default() -> Self {
        Self::new(PklCli::default())
    }
}

impl ProjectManager {
    pub fn new(cli: PklCli) -> Self {
        Self {
            cli,
            workspace_folders: Vec::new(),
            projects: BTreeMap::new(),
            excluded_directories: SKIPPED_DIRS.iter().map(|name| (*name).to_owned()).collect(),
        }
    }

    /// Apply settings without discarding synced project metadata.
    pub fn configure(&mut self, cli: PklCli, excluded_directories: Vec<String>) {
        self.cli = cli;
        self.excluded_directories = excluded_directories;
    }

    /// Set the workspace roots
    pub fn initialize(&mut self, folders: Vec<PathBuf>) {
        self.workspace_folders = folders;
        self.projects.clear();
    }

    /// Workspace roots currently managed by this project manager.
    pub fn workspace_folders(&self) -> &[PathBuf] {
        &self.workspace_folders
    }

    /// Which configured workspace owns this project? The most specific root
    /// wins when workspace folders are nested.
    fn project_belongs_to_workspace(&self, file: &Path, workspace: &Path) -> bool {
        if self.workspace_folders.is_empty() {
            return file.starts_with(workspace);
        }
        self.workspace_folders
            .iter()
            .filter(|root| file.starts_with(root))
            .max_by_key(|root| root.components().count())
            .is_some_and(|root| root == workspace)
    }

    /// Find `PklProject` files without invoking the `pkl` CLI.
    ///
    /// Keeping discovery separate makes filesystem traversal independently
    /// testable; [`Self::sync_projects`] will orchestrate it with CLI calls.
    pub fn discover_project_files(&self) -> Result<Vec<PathBuf>, ProjectError> {
        let mut project_files = Vec::new();
        for workspace_folder in &self.workspace_folders {
            walk_project_files(
                workspace_folder,
                &mut project_files,
                &self.excluded_directories,
            )?;
        }
        project_files.sort();
        project_files.dedup();
        Ok(project_files)
    }

    /// Discover all `.pkl-project` files under the workspace folders, run
    /// `pkl project resolve`, and parse the resulting metadata + deps files.
    pub fn sync_projects(&mut self) -> Result<(), ProjectError> {
        let project_files = self.discover_project_files()?;
        let mut synced = BTreeMap::new();
        if !project_files.is_empty() {
            let mut project_dirs: Vec<PathBuf> = project_files
                .iter()
                .filter_map(|file| file.parent().map(Path::to_path_buf))
                .collect();
            project_dirs.sort();
            project_dirs.dedup();
            self.cli.resolve_project(&project_dirs)?;
            let projects_metadata =
                self.cli
                    .eval(&project_files, Some(DEPENDENCIES_EXPR), Some(", "))?;
            let projects = PklProject::parse_metadata(&format!("[{projects_metadata}]"))?;
            for mut project in projects {
                let project_dir = project.project_dir().ok_or(ProjectError::MissingPath)?;
                let project_file = project_dir.join("PklProject");
                if !self
                    .workspace_folders
                    .iter()
                    .any(|root| project_file.starts_with(root))
                {
                    return Err(ProjectError::InvalidStatePath(project_file));
                }
                let project_deps = project_dir.join("PklProject.deps.json");
                if project_deps.try_exists()? {
                    let deps_json = std::fs::read_to_string(project_deps)?;
                    project.project_deps = Some(PklProject::load_project_deps(&deps_json)?);
                }
                synced.insert(project_file, Arc::new(project));
            }
        }
        self.projects = synced;
        Ok(())
    }

    /// The innermost project containing `file`, if any.
    pub fn get_pkl_project(&self, file: &Path) -> Option<&PklProject> {
        for parent in file.ancestors() {
            if let Some(project) = self.projects.get(&parent.join("PklProject")) {
                return Some(project);
            };
        }
        None
    }

    /// Owned metadata for publishing database project inputs.
    pub fn owned_projects(&self) -> Vec<Arc<PklProject>> {
        self.projects.values().cloned().collect()
    }

    /// Install a watched project input already read and decoded by a blocking worker.
    /// Missing or invalid dependency contents are represented by None, clearing stale data.
    /// Project changes invalidate metadata until the next explicit CLI sync.
    pub fn apply_watched_file(&mut self, path: &Path, deps: Option<ProjectDeps>) -> bool {
        match path.file_name().and_then(|name| name.to_str()) {
            Some("PklProject") => self.projects.remove(path).is_some(),
            Some("PklProject.deps.json") => {
                let Some(parent) = path.parent() else {
                    return false;
                };
                let Some(project) = self.projects.get_mut(&parent.join("PklProject")) else {
                    return false;
                };
                Arc::make_mut(project).project_deps = deps;
                true
            }
            _ => false,
        }
    }

    /// All discovered projects.
    pub fn projects(&self) -> impl Iterator<Item = &PklProject> {
        self.projects.values().map(Arc::as_ref)
    }

    /// Persist current state to `state_file` so restarts skip re-resolution.
    /// Project paths in the JSON are relative to the workspace root; a file
    /// under `.lspkl/` uses its parent as the root.
    pub fn persist_state(&self, state_file: &Path) -> Result<(), ProjectError> {
        let workspace = state_workspace_root(state_file);
        let pkl_projects = self
            .projects
            .iter()
            .filter(|(file, _)| self.project_belongs_to_workspace(file, workspace))
            .map(|(file, project)| {
                let relative = file
                    .strip_prefix(workspace)
                    .map_err(|_| ProjectError::InvalidStatePath(file.clone()))?;
                let project_file = relative
                    .to_str()
                    .ok_or_else(|| ProjectError::InvalidStatePath(file.clone()))?
                    .replace(std::path::MAIN_SEPARATOR, "/");
                Ok(crate::dto::PklWorkspaceStateEntry {
                    project_file,
                    package_uri: project.metadata.package_uri.clone(),
                    declared_dependencies: project.metadata.declared_dependencies.clone(),
                    resolved_dependencies: project
                        .project_deps
                        .as_ref()
                        .map(|deps| deps.resolved_dependencies.clone()),
                    evaluator_settings: project.metadata.evaluator_settings.clone(),
                })
            })
            .collect::<Result<Vec<_>, ProjectError>>()?;
        let state = PklWorkspaceState {
            schema_version: 1,
            pkl_projects,
        };
        let json = serde_json::to_vec_pretty(&state)?;
        if let Some(parent) = state_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(state_file, json)?;
        Ok(())
    }

    /// Replace only this workspace's projects with those in `state_file`.
    /// Projects restored from other workspace state files stay registered.
    pub fn load_state(&mut self, state_file: &Path) -> Result<(), ProjectError> {
        let state: PklWorkspaceState = serde_json::from_slice(&std::fs::read(state_file)?)?;
        if state.schema_version != 1 {
            return Err(ProjectError::UnsupportedStateVersion(state.schema_version));
        }
        let workspace = state_workspace_root(state_file);
        let mut projects = BTreeMap::new();
        for entry in state.pkl_projects {
            let relative = Path::new(&entry.project_file);
            if relative.is_absolute()
                || relative
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
            {
                return Err(ProjectError::InvalidStatePath(relative.to_path_buf()));
            }
            let project_file = workspace.join(relative);
            if !self.project_belongs_to_workspace(&project_file, workspace) {
                return Err(ProjectError::InvalidStatePath(project_file));
            }
            let project_file_uri = url::Url::from_file_path(&project_file)
                .map_err(|_| ProjectError::InvalidStatePath(project_file.clone()))?
                .to_string();
            let project_deps =
                entry
                    .resolved_dependencies
                    .map(|resolved_dependencies| ProjectDeps {
                        schema_version: 1,
                        resolved_dependencies,
                    });
            let metadata = DerivedProjectMetadata {
                project_file_uri,
                package_uri: entry.package_uri,
                declared_dependencies: entry.declared_dependencies,
                evaluator_settings: entry.evaluator_settings,
            };
            projects.insert(
                project_file,
                Arc::new(PklProject {
                    metadata,
                    project_deps,
                }),
            );
        }
        let replaced: Vec<_> = self
            .projects
            .keys()
            .filter(|file| self.project_belongs_to_workspace(file, workspace))
            .cloned()
            .collect();
        for file in replaced {
            self.projects.remove(&file);
        }
        self.projects.extend(projects);
        Ok(())
    }
}
