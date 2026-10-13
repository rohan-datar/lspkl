//! Workspace settings consumed by project resolution and the formatter.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
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
    /// Initialized once, and retained when workspace-relative paths are rebound.
    #[serde(skip)]
    pub fallback_cache_dir: PathBuf,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            modulepath: Vec::new(),
            cli: CliSettings::default(),
            projects: ProjectSettings::default(),
            formatter: FormatterSettings::default(),
            cache_dir: None,
            module_paths: Vec::new(),
            resolved_cache_dir: None,
            fallback_cache_dir: std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".pkl/cache"))
                .unwrap_or_else(pkl_vfs::default_cache),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct CliSettings {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectSettings {
    pub excluded_directories: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FormatterSettings {
    pub grammar_version: Option<GrammarVersion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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
            .map(|path| {
                if path.is_absolute() {
                    path.clone()
                } else {
                    workspaces
                        .iter()
                        .map(|root| root.join(path))
                        .find(|path| path.exists())
                        .unwrap_or_else(|| {
                            workspaces
                                .first()
                                .map_or_else(|| path.clone(), |root| root.join(path))
                        })
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
            fallback_cache_dir: self.fallback_cache_dir.clone(),
        };
        settings.resolve_paths(workspaces);
        settings
    }

    /// Borrow the resolved override or the fallback captured when settings were created.
    pub fn effective_cache_dir(&self) -> &Path {
        self.resolved_cache_dir
            .as_deref()
            .unwrap_or(&self.fallback_cache_dir)
    }

    pub fn cli_path(&self) -> &Path {
        self.cli.path.as_deref().unwrap_or_else(|| Path::new("pkl"))
    }
}

#[cfg(test)]
mod tests {
    use super::WorkspaceSettings;
    use std::path::PathBuf;

    #[test]
    fn deserialization_initializes_fallback_and_ignores_runtime_fields() {
        let settings = WorkspaceSettings::parse(serde_json::json!({
            "pkl": {
                "cacheDir": "custom-cache",
                "fallbackCacheDir": "untrusted-fallback",
                "resolvedCacheDir": "untrusted-resolved"
            }
        }))
        .unwrap();
        assert_eq!(settings.cache_dir, Some(PathBuf::from("custom-cache")));
        assert_eq!(settings.resolved_cache_dir, None);
        assert_eq!(
            settings.fallback_cache_dir,
            WorkspaceSettings::default().fallback_cache_dir,
        );
        assert_eq!(settings.effective_cache_dir(), settings.fallback_cache_dir);
    }

    #[test]
    fn resolved_override_wins_and_rebind_preserves_fallback() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let fallback = first.path().join("captured-fallback");
        let mut settings = WorkspaceSettings {
            cache_dir: Some(PathBuf::from("custom-cache")),
            fallback_cache_dir: fallback.clone(),
            ..Default::default()
        };
        settings.resolve_paths(&[first.path().to_owned()]);
        assert_eq!(
            settings.effective_cache_dir(),
            first.path().join("custom-cache"),
        );

        let rebound = settings.rebind(&[second.path().to_owned()]);
        assert_eq!(rebound.fallback_cache_dir, fallback);
        assert_eq!(
            rebound.effective_cache_dir(),
            second.path().join("custom-cache"),
        );

        settings.cache_dir = None;
        let rebound = settings.rebind(&[second.path().to_owned()]);
        assert_eq!(rebound.effective_cache_dir(), fallback);
    }

    #[test]
    fn absolute_override_is_not_rebound() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("cache");
        let settings = WorkspaceSettings {
            cache_dir: Some(cache.clone()),
            ..Default::default()
        };
        assert_eq!(settings.rebind(&[]).effective_cache_dir(), cache);
        assert_eq!(
            settings
                .rebind(&[root.path().join("other")])
                .effective_cache_dir(),
            cache,
        );
    }

    #[test]
    fn modulepath_binding_retains_missing_paths_and_prefers_first_existing_workspace() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        std::fs::create_dir(second.path().join("present")).unwrap();
        let mut settings = WorkspaceSettings {
            modulepath: vec!["missing".into(), "present".into()],
            ..Default::default()
        };
        settings.resolve_paths(&[first.path().to_owned(), second.path().to_owned()]);
        assert_eq!(
            settings.module_paths,
            [first.path().join("missing"), second.path().join("present")]
        );
        settings.resolve_paths(&[]);
        assert_eq!(
            settings.module_paths,
            [PathBuf::from("missing"), PathBuf::from("present")]
        );
    }
}
