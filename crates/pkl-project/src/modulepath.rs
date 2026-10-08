//! `modulepath:` URI resolution.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use pkl_vfs::FileManager;
use pkl_vfs::file_manager::url_to_path;

use crate::dto::PklProject;

/// Resolves `modulepath:` URIs against a project's modulepath entries.
/// Fields come alive as the todo!() bodies are filled in.
#[allow(dead_code)]
pub struct ModulepathResolver<'a> {
    file_manager: &'a FileManager,
}

impl<'a> ModulepathResolver<'a> {
    pub fn new(file_manager: &'a FileManager) -> Self {
        Self { file_manager }
    }

    /// Effective directory roots for `context`. Archives and global settings
    /// are not included in this first resolver slice.
    pub fn module_paths(&self, context: Option<&PklProject>) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        let Some(ctx) = context else { return paths };
        let Some(project_dir) = ctx.project_dir() else {
            return paths;
        };
        let Some(evaluator_settings) = ctx.metadata.evaluator_settings.as_ref() else {
            return paths;
        };
        let Some(module_paths) = evaluator_settings.module_path.as_ref() else {
            return paths;
        };

        for module_path in module_paths {
            paths.push(normalize_path(&project_dir.join(module_path)));
        }
        paths
    }

    /// Search directory roots in declaration order for a relative module path.
    fn resolve_in_roots(
        &self,
        relative_path: &Path,
        context: Option<&PklProject>,
    ) -> Option<Arc<pkl_vfs::VirtualFile>> {
        // Normalize before joining a root: `nested/../shared.pkl` should
        // find a root-level file even if that root has no `nested/` directory.
        if relative_path.is_absolute() {
            return None;
        }
        let relative_path = normalize_path(relative_path);
        self.module_paths(context).into_iter().find_map(|root| {
            let candidate = root.join(&relative_path);
            if !candidate.is_file() {
                return None;
            }
            self.file_manager.get_modulepath_file(&candidate).ok()
        })
    }

    /// Resolve `path` (the part after `modulepath:`) against `context`'s
    /// modulepath. Returns the first regular file found in declaration order.
    /// Kotlin reference: `resolveAbsolute`.
    pub fn resolve_absolute(
        &self,
        path: &str,
        context: Option<&PklProject>,
    ) -> Option<Arc<pkl_vfs::VirtualFile>> {
        self.resolve_in_roots(Path::new(path.strip_prefix('/')?), context)
    }

    /// Resolve `path` relative to an already-known modulepath `file`.
    pub fn resolve_relative(
        &self,
        file: &pkl_vfs::VirtualFile,
        path: &str,
        context: Option<&PklProject>,
    ) -> Option<Arc<pkl_vfs::VirtualFile>> {
        if path.starts_with('/') {
            return self.resolve_absolute(path, context);
        }
        let file_path = url_to_path(&file.url).ok()?;
        let roots = self.module_paths(context);
        let relative_source = roots
            .iter()
            .find_map(|root| file_path.strip_prefix(root).ok())?;
        let relative_target = relative_source.parent()?.join(path);
        self.resolve_in_roots(&relative_target, context)
    }

    /// Is `file` reachable through any project's modulepath?
    pub fn is_on_module_path(&self, file: &Path, context: Option<&PklProject>) -> bool {
        let roots = self.module_paths(context);
        roots.iter().any(|root| file.starts_with(root))
    }
}

/// Lexically simplify a path without requiring intermediate directories to
/// exist. Preserve leading `..` for relative imports, but never pop a root.
fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.file_name().is_some_and(|last| last != "..") {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push("..");
                }
            }
        }
    }
    normalized
}
