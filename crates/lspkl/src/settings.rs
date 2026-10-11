//! Workspace settings consumed by project resolution and the formatter.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceSettings {
    pub modulepath: Vec<PathBuf>,
    pub cli: CliSettings,
    pub projects: ProjectSettings,
    pub formatter: FormatterSettings,
    pub cache_dir: Option<PathBuf>,
    #[serde(skip)]
    pub module_paths: Vec<PathBuf>,
    #[serde(skip)]
    pub resolved_cache_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CliSettings {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectSettings {
    pub excluded_directories: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FormatterSettings {
    pub grammar_version: Option<GrammarVersion>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub enum GrammarVersion {
    #[serde(rename = "1")]
    V1,
    #[serde(rename = "2")]
    V2,
}

impl GrammarVersion {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::V1 => "1",
            Self::V2 => "2",
        }
    }
}

impl WorkspaceSettings {
    pub fn parse(mut value: Value) -> Result<Self, serde_json::Error> {
        let value = match value.get_mut("pkl") {
            Some(section) => section.take(),
            None => value,
        };
        serde_json::from_value(value)
    }

    /// Keep declared paths so workspace-folder changes can resolve them again.
    pub fn resolve_paths(&mut self, workspaces: &[PathBuf]) {
        self.module_paths = self
            .modulepath
            .iter()
            .filter_map(|path| {
                if path.is_absolute() {
                    Some(path.clone())
                } else {
                    workspaces
                        .iter()
                        .map(|root| root.join(path))
                        .find(|path| path.exists())
                }
            })
            .collect();
        self.resolved_cache_dir = self.cache_dir.as_ref().map(|path| {
            if !path.is_absolute()
                && let Some(root) = workspaces.first()
            {
                root.join(path)
            } else {
                path.clone()
            }
        });
    }

    pub fn rebind(&self, workspaces: &[PathBuf]) -> Self {
        let mut settings = Self {
            modulepath: self.modulepath.clone(),
            cli: self.cli.clone(),
            projects: self.projects.clone(),
            formatter: self.formatter.clone(),
            cache_dir: self.cache_dir.clone(),
            module_paths: Vec::new(),
            resolved_cache_dir: None,
        };
        settings.resolve_paths(workspaces);
        settings
    }

    pub fn cli_path(&self) -> &Path {
        self.cli.path.as_deref().unwrap_or_else(|| Path::new("pkl"))
    }
}
