//! Network-free module routing and completion candidates supplied by project/source state.
use crate::PklProject;
use fluent_uri::Uri;
use lspkl_vfs::{FileError, Source};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

/// Project-aware routing boundary shared by database queries and LSP completion.
/// Candidate discovery does not load or parse candidate module contents.
pub trait ModuleResolver {
    fn resolve_file(
        &self,
        reference: &str,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Option<Arc<Source>>;
    fn resolve_glob(
        &self,
        reference: &str,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Vec<Arc<Source>>;
    fn missing_package_uri(
        &self,
        reference: &str,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Option<String>;
    fn completion_root(
        &self,
        reference: &str,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Option<Uri<String>>;
    fn dependency_names(&self, source: &Source, context: Option<&PklProject>) -> Vec<String>;
    fn modulepath_relative_path(
        &self,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Option<String>;
    fn modulepath_roots(&self, context: Option<&PklProject>) -> Vec<Uri<String>>;
    fn list_files(&self, directory: &Uri<String>) -> Result<Vec<Uri<String>>, FileError>;
    fn resolve_uri(&self, base: &Uri<String>, reference: &str) -> Result<Uri<String>, FileError>;
    fn cache_dir(&self) -> &Path;
    fn cached_packages(&self) -> BTreeSet<String> {
        cached_packages(self.cache_dir())
    }
    fn children(&self, directory: &Uri<String>) -> Vec<(String, bool, Uri<String>)> {
        children(self, directory)
    }
    fn modulepath_children(
        &self,
        parent: &str,
        context: Option<&PklProject>,
    ) -> Vec<(String, bool, Uri<String>)> {
        modulepath_children(self, parent, context)
    }
    fn upward_directories(&self, source: &Source, parent: &str) -> Vec<Uri<String>> {
        let Ok(path) = lspkl_vfs::url_to_path(&source.url) else {
            return Vec::new();
        };
        let Some(directory) = path.parent() else {
            return Vec::new();
        };
        let suffix = parent.strip_prefix(".../").unwrap_or("");
        directory
            .ancestors()
            .skip(1)
            .filter_map(|ancestor| file_uri(&ancestor.join(suffix), true))
            .collect()
    }
}

fn logical_children(
    files: BTreeMap<String, Uri<String>>,
    parent: &str,
) -> Vec<(String, bool, Uri<String>)> {
    let mut children = BTreeMap::new();
    for (path, uri) in files {
        let Some(rest) = path.strip_prefix(parent) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        let (name, folder) = rest.find('/').map_or((rest.to_owned(), false), |i| {
            (rest[..i + 1].to_owned(), true)
        });
        let target = if folder {
            let encoded = encode_path(&rest[name.len()..]);
            Uri::parse(
                uri.as_str()
                    .strip_suffix(&encoded)
                    .unwrap_or(uri.as_str())
                    .to_owned(),
            )
            .unwrap_or(uri)
        } else {
            uri
        };
        children.entry(name).or_insert((folder, target));
    }
    children
        .into_iter()
        .map(|(name, (folder, uri))| (name, folder, uri))
        .collect()
}
fn children(
    resolver: &(impl ModuleResolver + ?Sized),
    directory: &Uri<String>,
) -> Vec<(String, bool, Uri<String>)> {
    if let Ok(path) = lspkl_vfs::url_to_path(directory) {
        return std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let folder = entry.file_type().ok()?.is_dir();
                let mut name = entry.file_name().into_string().ok()?;
                if folder {
                    name.push('/');
                }
                Some((name, folder, file_uri(&entry.path(), folder)?))
            })
            .collect();
    }
    let mut files = BTreeMap::new();
    if let Ok(uris) = resolver.list_files(directory) {
        for uri in uris {
            if let Some(relative) = uri
                .as_str()
                .strip_prefix(directory.as_str())
                .and_then(decode_uri_path)
            {
                files.insert(relative, uri);
            }
        }
    }
    logical_children(files, "")
}
fn file_uri(path: &Path, directory: bool) -> Option<Uri<String>> {
    let uri = lspkl_vfs::path_to_uri(path).ok()?;
    if directory && !uri.as_str().ends_with('/') {
        let mut value = uri.into_string();
        value.push('/');
        Uri::parse(value).ok()
    } else {
        Some(uri)
    }
}
use lspkl_vfs::{decode_uri_path, encode_path};
fn cached_packages(cache: &Path) -> BTreeSet<String> {
    let mut packages = BTreeSet::new();
    for layout in ["package-1", "package-2"] {
        let root = cache.join(layout);
        let mut pending = vec![root.clone()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    pending.push(entry.path());
                } else if entry.path().extension().is_some_and(|e| e == "json") {
                    let path = entry.path();
                    let Some(relative) = path
                        .parent()
                        .and_then(|dir| dir.strip_prefix(&root).ok())
                        .and_then(Path::to_str)
                    else {
                        continue;
                    };
                    // Cache paths are native filesystem paths; package URIs use `/`.
                    #[cfg(windows)]
                    let normalized = relative.replace('\\', "/");
                    #[cfg(windows)]
                    let relative = normalized.as_str();
                    let Some(relative) = decode_cache_path(relative, layout == "package-2") else {
                        continue;
                    };
                    let value = format!("package://{relative}");
                    if crate::dto::PackageUri::create(&value).is_ok() {
                        packages.insert(value);
                    }
                }
            }
        }
    }
    packages
}
fn decode_cache_path(path: &str, encoded: bool) -> Option<String> {
    if !encoded {
        return Some(path.to_owned());
    }
    let mut value = String::new();
    let mut rest = path;
    while let Some(index) = rest.find('(') {
        value.push_str(&rest[..index]);
        rest = &rest[index + 1..];
        if let Some(next) = rest.strip_prefix('(') {
            value.push('(');
            rest = next;
        } else {
            let end = rest.find(')')?;
            let code = u32::from_str_radix(&rest[..end], 16).ok()?;
            value.push(char::from_u32(code)?);
            rest = &rest[end + 1..];
        }
    }
    value.push_str(rest);
    Some(value)
}
fn modulepath_children(
    resolver: &(impl ModuleResolver + ?Sized),
    parent: &str,
    project: Option<&PklProject>,
) -> Vec<(String, bool, Uri<String>)> {
    let mut entries = BTreeMap::new();
    for root in resolver.modulepath_roots(project) {
        let Ok(directory) = resolver.resolve_uri(&root, &format!("./{}", encode_path(parent)))
        else {
            continue;
        };
        for (name, folder, target) in resolver.children(&directory) {
            entries.entry(name).or_insert((folder, target));
        }
    }
    entries
        .into_iter()
        .map(|(name, (folder, target))| (name, folder, target))
        .collect()
}
