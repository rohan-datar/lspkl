//! Ordered directory and archive-backed `modulepath:` resolution.

use crate::dto::PklProject;
use fluent_uri::Uri;
use pkl_vfs::{Source, SourceLoader, effective_uri, url_to_path};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

pub struct ModulepathResolver<'a> {
    loader: &'a dyn SourceLoader,
    configured: &'a [PathBuf],
    cache_dir: &'a Path,
}

impl<'a> ModulepathResolver<'a> {
    pub fn new(loader: &'a dyn SourceLoader, cache_dir: &'a Path) -> Self {
        Self::with_modulepaths(loader, &[], cache_dir)
    }
    pub fn with_modulepaths(
        loader: &'a dyn SourceLoader,
        configured: &'a [PathBuf],
        cache_dir: &'a Path,
    ) -> Self {
        Self {
            loader,
            configured,
            cache_dir,
        }
    }
    /// Project entries precede globally configured entries.
    pub fn module_paths(&self, context: Option<&PklProject>) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Some(ctx) = context
            && let Some(dir) = ctx.project_dir()
            && let Some(settings) = &ctx.metadata.evaluator_settings
            && let Some(roots) = &settings.module_path
        {
            paths.extend(roots.iter().map(|root| normalize_path(&dir.join(root))));
        }
        paths.extend(self.configured.iter().map(|root| {
            normalize_path(&if root.is_absolute() {
                root.clone()
            } else {
                std::env::current_dir().unwrap_or_default().join(root)
            })
        }));
        paths
    }
    pub fn roots(&self, context: Option<&PklProject>) -> Vec<Uri<String>> {
        self.module_paths(context)
            .iter()
            .filter_map(|path| root_uri(path))
            .collect()
    }
    pub fn relative_path(&self, file: &Source, context: Option<&PklProject>) -> Option<String> {
        let uri = effective_uri(&file.url).ok()?;
        self.roots(context)
            .iter()
            .find_map(|root| relative_uri(root, &uri))
    }
    pub fn resolve_absolute(
        &self,
        path: &str,
        context: Option<&PklProject>,
    ) -> Option<Arc<Source>> {
        let path = path.strip_prefix('/')?;
        self.roots(context).iter().find_map(|root| {
            let target = resolve_member(self.loader, root, path)?;
            self.loader.load(&target, self.cache_dir).ok()
        })
    }
    pub fn resolve_relative(
        &self,
        file: &Source,
        path: &str,
        context: Option<&PklProject>,
    ) -> Option<Arc<Source>> {
        if path.starts_with('/') {
            return self.resolve_absolute(path, context);
        }
        let relative = self.relative_path(file, context)?;
        let parent = relative.rsplit_once('/').map_or("", |(parent, _)| parent);
        self.resolve_absolute(
            &format!(
                "/{}",
                if parent.is_empty() {
                    path.to_owned()
                } else {
                    format!("{parent}/{path}")
                }
            ),
            context,
        )
    }
    pub fn is_on_module_path(&self, file: &Path, context: Option<&PklProject>) -> bool {
        self.module_paths(context)
            .iter()
            .any(|root| root.is_dir() && file.starts_with(root))
    }
    /// Merge by logical path, retaining the first root's file for shadowed entries.
    pub fn files(&self, context: Option<&PklProject>) -> BTreeMap<String, Uri<String>> {
        let mut files = BTreeMap::new();
        for root in self.roots(context) {
            if let Ok(entries) = self.loader.list_files(&root) {
                for uri in entries {
                    if let Some(path) = relative_uri(&root, &uri) {
                        files.entry(path).or_insert(uri);
                    }
                }
            }
        }
        files
    }
}

pub(crate) fn root_uri(path: &Path) -> Option<Uri<String>> {
    let mut url = pkl_vfs::path_to_uri(path).ok()?.into_string();
    let archive = path.is_file()
        && (matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("zip" | "jar")
        ) || {
            let mut header = [0; 64];
            std::fs::File::open(path)
                .and_then(|mut f| f.read(&mut header))
                .is_ok_and(|n| {
                    header[..n].starts_with(b"PK\x03\x04")
                        || header[..n].starts_with(b"#!/bin/sh\n      exec java  -jar $0 \"$@\"")
                })
        });
    if archive {
        url = format!("jar:{url}!/");
    } else if !url.ends_with('/') {
        url.push('/');
    }
    Uri::parse(url).ok()
}

pub(crate) fn relative_uri(root: &Uri<String>, file: &Uri<String>) -> Option<String> {
    if root.scheme().as_str() == "file" && file.scheme().as_str() == "file" {
        let file = url_to_path(file).ok()?;
        let root = url_to_path(root).ok()?;
        let relative = file.strip_prefix(root).ok()?.to_string_lossy();
        return Some(if cfg!(windows) {
            relative.replace('\\', "/")
        } else {
            relative.into_owned()
        });
    }
    let root = effective_uri(root).ok()?;
    let file = effective_uri(file).ok()?;
    percent_encoding::percent_decode_str(file.as_str().strip_prefix(root.as_str())?)
        .decode_utf8()
        .ok()
        .map(|s| s.into_owned())
}

/// Decode a URI path once; callers match logical filenames, never encoded bytes.
pub(crate) fn decode_path(path: &str) -> Option<std::borrow::Cow<'_, str>> {
    percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()
}

/// A dependency member is a confined path, never a URI reference. Prefixing
/// its re-encoded normalized path prevents VFS scheme/authority dispatch and
/// ensures literal percent signs are not decoded a second time.
pub(crate) fn resolve_member(
    files: &dyn SourceLoader,
    root: &Uri<String>,
    path: &str,
) -> Option<Uri<String>> {
    let decoded = decode_path(path)?;
    if decoded.contains(['\\', '\0']) {
        return None;
    }
    let path = normalize_relative(&decoded)?;
    use std::fmt::Write;
    let mut encoded = String::from("./");
    for (index, part) in path.split('/').enumerate() {
        if index != 0 {
            encoded.push('/');
        }
        write!(
            encoded,
            "{}",
            percent_encoding::utf8_percent_encode(part, percent_encoding::NON_ALPHANUMERIC)
        )
        .ok()?;
    }
    files.resolve_uri(root, &encoded).ok()
}

pub(crate) fn normalize_relative(path: &str) -> Option<String> {
    if path.starts_with('/') || path.contains('\0') || path.split('/').next()?.contains(':') {
        return None;
    }
    // These are URI/glob paths, not native filesystem paths.
    let mut normalized = String::with_capacity(path.len());
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if normalized.is_empty() {
                    return None;
                }
                normalized.truncate(normalized.rfind('/').unwrap_or(0));
            }
            _ => {
                if !normalized.is_empty() {
                    normalized.push('/');
                }
                normalized.push_str(part);
            }
        }
    }
    if normalized.split('/').next()?.contains(':') {
        return None;
    }
    Some(normalized)
}
fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str())
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
