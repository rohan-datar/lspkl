//! Module URI routing, dependency lookup, and deterministic glob expansion.
use crate::dto::{
    Dependency, PackageDependency, PackageMetadata, PackageUri, PklProject, resolved_to_dependency,
};
use crate::modulepath::{
    ModulepathResolver, decode_path, normalize_relative, relative_uri, resolve_member,
};
use fluent_uri::{Uri, UriRef};
use pkl_vfs::{Source, SourceLoader, effective_uri, url_to_path};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct ImportResolver<'a> {
    loader: &'a dyn SourceLoader,
    modulepath: ModulepathResolver<'a>,
    cache_dir: &'a Path,
    facts: &'a dyn crate::routing::RoutingFacts,
}
impl<'a> ImportResolver<'a> {
    pub fn new(loader: &'a dyn SourceLoader, cache_dir: &'a Path) -> Self {
        Self {
            loader,
            modulepath: ModulepathResolver::new(loader, cache_dir),
            cache_dir,
            facts: &crate::routing::DiskRoutingFacts,
        }
    }
    pub fn with_settings(
        loader: &'a dyn SourceLoader,
        modulepaths: &'a [PathBuf],
        cache_dir: &'a Path,
    ) -> Self {
        Self {
            loader,
            modulepath: ModulepathResolver::with_modulepaths(loader, modulepaths, cache_dir),
            cache_dir,
            facts: &crate::routing::DiskRoutingFacts,
        }
    }
    pub fn with_facts(mut self, facts: &'a dyn crate::routing::RoutingFacts) -> Self {
        self.facts = facts;
        self.modulepath.facts = facts;
        self
    }
    fn dependency_root(&self, dependency: Dependency) -> Option<Uri<String>> {
        match dependency {
            Dependency::LocalProject(dep) => self
                .facts
                .root(crate::routing::RoutingRequest::ModuleRoot(dep.project_dir)),
            Dependency::Package(dep) => self
                .facts
                .root(crate::routing::package_request(&dep, self.cache_dir)),
        }
    }
    fn resolve_dependency(
        &self,
        uri: &PackageUri,
        context: Option<&PklProject>,
    ) -> Option<Dependency> {
        let project = context?;
        let resolved = project
            .project_deps
            .as_ref()?
            .get_resolved_dependency(&uri.to_display_string())?;
        Some(resolved_to_dependency(resolved, &project.project_dir()?))
    }
    fn named_dependencies(
        &self,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Option<std::collections::BTreeMap<String, Dependency>> {
        let source_uri = effective_uri(&source.url).ok()?;
        if source_uri.scheme().as_str() == "jar" {
            let archive = source_uri
                .as_str()
                .strip_prefix("jar:")?
                .split_once("!/")?
                .0;
            let path = url_to_path(&Uri::parse(archive.to_owned()).ok()?).ok()?;
            let metadata_uri = pkl_vfs::path_to_uri(&path.with_extension("json")).ok()?;
            let json = self.loader.load(&metadata_uri, self.cache_dir).ok()?;
            let metadata: PackageMetadata = serde_json::from_str(json.contents()).ok()?;
            return Some(
                metadata
                    .dependencies
                    .into_iter()
                    .map(|(name, dep)| {
                        let dependency = self.resolve_dependency(&dep.uri, context).unwrap_or(
                            Dependency::Package(PackageDependency {
                                uri: dep.uri,
                                checksums: dep.checksums,
                            }),
                        );
                        (name, dependency)
                    })
                    .collect(),
            );
        }
        let project = context?;
        Some(
            project
                .metadata
                .declared_dependencies
                .iter()
                .map(|(name, uri)| {
                    let dep = self.resolve_dependency(uri, context).unwrap_or_else(|| {
                        Dependency::Package(PackageDependency {
                            uri: uri.clone(),
                            checksums: uri.checksums.clone(),
                        })
                    });
                    (name.clone(), dep)
                })
                .collect(),
        )
    }

    fn reference_dependency(
        &self,
        reference: &str,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Option<Dependency> {
        if reference.starts_with("package:") {
            let uri =
                PackageUri::create(reference.split_once('#').map_or(reference, |(uri, _)| uri))
                    .ok()?;
            return Some(self.resolve_dependency(&uri, context).unwrap_or_else(|| {
                Dependency::Package(PackageDependency {
                    checksums: uri.checksums.clone(),
                    uri,
                })
            }));
        }
        let name = reference.strip_prefix('@')?.split('/').next()?;
        self.named_dependencies(source, context)?.remove(name)
    }

    fn package_root(
        &self,
        reference: &str,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Option<Uri<String>> {
        self.dependency_root(self.reference_dependency(reference, source, context)?)
    }

    /// Return a download target only for a known remote package lacking cached sources.
    /// A cached package with a nonexistent member never yields a download action.
    pub fn missing_package_uri(
        &self,
        reference: &str,
        source: &Source,
        project: Option<&PklProject>,
    ) -> Option<String> {
        let Dependency::Package(dep) = self.reference_dependency(reference, source, project)?
        else {
            return None;
        };
        let root = self
            .facts
            .root(crate::routing::package_request(&dep, self.cache_dir));
        root.is_none().then(|| {
            let mut uri = dep.uri.to_display_string();
            if let Some(checksums) = dep.checksums.as_ref().or(dep.uri.checksums.as_ref()) {
                uri.push_str("::sha256:");
                uri.push_str(&checksums.sha256);
            }
            uri
        })
    }

    /// Cached dependency root for module URI completion.
    pub fn completion_root(
        &self,
        reference: &str,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Option<Uri<String>> {
        self.package_root(reference, source, context)
    }

    /// Dependency names visible to this source, including cached package metadata.
    pub fn dependency_names(&self, source: &Source, context: Option<&PklProject>) -> Vec<String> {
        self.named_dependencies(source, context)
            .map(|deps| deps.into_keys().collect())
            .unwrap_or_default()
    }

    pub fn resolve_file(
        &self,
        reference: &str,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Option<Arc<Source>> {
        let uri = UriRef::parse(reference).ok()?;
        let source_uri = effective_uri(&source.url).ok()?;
        let scheme = uri.scheme().map(|s| s.as_str());
        let target = match scheme {
            Some("modulepath") => {
                return self
                    .modulepath
                    .resolve_absolute(uri.path().as_str(), context);
            }
            Some("package") => {
                let fragment = uri.fragment()?.as_str();
                if !fragment.starts_with('/') {
                    return None;
                }
                let root = self.package_root(reference, source, context)?;
                resolve_member(self.loader, &root, fragment.strip_prefix('/')?)?
            }
            Some("file") if source_uri.scheme().as_str() != "file" => return None,
            Some("file" | "https" | "pkl") => Uri::parse(reference.to_owned()).ok()?,
            Some(_) => return None,
            None if source_uri.scheme().as_str() == "https" => {
                self.loader.resolve_uri(&source_uri, reference).ok()?
            }
            None if reference.starts_with('@') => {
                let root = self.package_root(reference, source, context)?;
                resolve_member(
                    self.loader,
                    &root,
                    reference.split_once('/').map_or("", |(_, path)| path),
                )?
            }
            None if reference == "..." || reference.starts_with(".../") => {
                return self.resolve_upward(reference, source, context);
            }
            None if self.modulepath.relative_path(source, context).is_some() => {
                return self
                    .modulepath
                    .resolve_relative(source, uri.path().as_str(), context);
            }
            None if matches!(source_uri.scheme().as_str(), "file" | "jar") => {
                self.loader.resolve_uri(&source_uri, reference).ok()?
            }
            None => return None,
        };
        self.loader.load(&target, self.cache_dir).ok()
    }
    fn resolve_upward(
        &self,
        reference: &str,
        source: &Source,
        _context: Option<&PklProject>,
    ) -> Option<Arc<Source>> {
        let uri = effective_uri(&source.url).ok()?;
        if !matches!(uri.scheme().as_str(), "file" | "jar") {
            return None;
        }
        let name = uri.as_str().rsplit('/').next()?;
        let path = reference.strip_prefix(".../").unwrap_or(name);
        let mut dir = self.loader.resolve_uri(&uri, "../").ok()?;
        loop {
            let target = resolve_member(self.loader, &dir, path)?;
            if target != uri
                && let Ok(file) = self.loader.load(&target, self.cache_dir)
            {
                return Some(file);
            }
            let parent = self.loader.resolve_uri(&dir, "../").ok()?;
            if parent == dir {
                return None;
            }
            dir = parent;
        }
    }
    /// Returns stable logical-path order; modulepath shadowing is applied before matching.
    pub fn resolve_glob(
        &self,
        reference: &str,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Vec<Arc<Source>> {
        self.glob(reference, source, context).unwrap_or_default()
    }
    fn glob(
        &self,
        reference: &str,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Option<Vec<Arc<Source>>> {
        if reference.starts_with("...") {
            return None;
        }
        // Glob metacharacters are legal Pkl paths even when URI syntax reserves them.
        let source_uri = effective_uri(&source.url).ok()?;
        let (root, pattern, modulepath) = if let Some(path) = reference.strip_prefix("modulepath:")
        {
            (
                None,
                decode_path(path.strip_prefix('/')?)?.into_owned(),
                true,
            )
        } else if reference.starts_with("package:") {
            let (_, fragment) = reference.split_once('#')?;
            (
                Some(self.package_root(reference, source, context)?),
                decode_path(fragment.strip_prefix('/')?)?.into_owned(),
                false,
            )
        } else if reference.starts_with('@') {
            (
                Some(self.package_root(reference, source, context)?),
                decode_path(reference.split_once('/')?.1)?.into_owned(),
                false,
            )
        } else if !reference.contains(':')
            && self.modulepath.relative_path(source, context).is_some()
        {
            let relative = self.modulepath.relative_path(source, context)?;
            let reference = decode_path(reference)?;
            let parent = relative.rsplit_once('/').map_or("", |(p, _)| p);
            (
                None,
                if reference.starts_with('/') {
                    reference.trim_start_matches('/').to_owned()
                } else if parent.is_empty() {
                    reference.into_owned()
                } else {
                    format!("{parent}/{reference}")
                },
                true,
            )
        } else {
            let path = if let Some(path) = reference.strip_prefix("file:") {
                if source_uri.scheme().as_str() != "file" {
                    return None;
                }
                if !path.starts_with('/') || path.starts_with("//") && !path.starts_with("///") {
                    return None;
                }
                format!("/{}", path.trim_start_matches('/'))
            } else {
                if reference.contains(':')
                    || !matches!(source_uri.scheme().as_str(), "file" | "jar")
                {
                    return None;
                }
                reference.to_owned()
            };
            let wildcard = path.find(['*', '?', '[', '{']).unwrap_or(path.len());
            let split = path[..wildcard].rfind('/').map_or(0, |i| i + 1);
            let root = self
                .loader
                .resolve_uri(&source_uri, &format!("{}./", &path[..split]))
                .ok()?;
            (Some(root), decode_path(&path[split..])?.into_owned(), false)
        };
        let pattern = normalize_relative(&pattern)?;
        let matcher = globset::GlobBuilder::new(&pattern)
            .literal_separator(true)
            .backslash_escape(true)
            .build()
            .ok()?
            .compile_matcher();
        let candidates = if modulepath {
            self.modulepath.files(context)
        } else {
            let root = root?;
            self.loader
                .list_files(&root)
                .ok()?
                .into_iter()
                .filter_map(|uri| Some((relative_uri(&root, &uri)?, uri)))
                .collect()
        };
        Some(
            candidates
                .into_iter()
                .filter(|(path, _)| matcher.is_match(path))
                .filter_map(|(_, uri)| self.loader.load(&uri, self.cache_dir).ok())
                .collect(),
        )
    }
}
impl crate::ModuleResolver for ImportResolver<'_> {
    fn resolve_file(
        &self,
        reference: &str,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Option<Arc<Source>> {
        ImportResolver::resolve_file(self, reference, source, context)
    }
    fn resolve_glob(
        &self,
        reference: &str,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Vec<Arc<Source>> {
        ImportResolver::resolve_glob(self, reference, source, context)
    }
    fn missing_package_uri(
        &self,
        reference: &str,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Option<String> {
        ImportResolver::missing_package_uri(self, reference, source, context)
    }
    fn completion_root(
        &self,
        reference: &str,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Option<Uri<String>> {
        ImportResolver::completion_root(self, reference, source, context)
    }
    fn dependency_names(&self, source: &Source, context: Option<&PklProject>) -> Vec<String> {
        ImportResolver::dependency_names(self, source, context)
    }
    fn modulepath_relative_path(
        &self,
        source: &Source,
        context: Option<&PklProject>,
    ) -> Option<String> {
        self.modulepath.relative_path(source, context)
    }
    fn modulepath_roots(&self, context: Option<&PklProject>) -> Vec<Uri<String>> {
        self.modulepath.roots(context)
    }
    fn list_files(&self, directory: &Uri<String>) -> Result<Vec<Uri<String>>, pkl_vfs::FileError> {
        self.loader.list_files(directory)
    }
    fn resolve_uri(
        &self,
        base: &Uri<String>,
        reference: &str,
    ) -> Result<Uri<String>, pkl_vfs::FileError> {
        self.loader.resolve_uri(base, reference)
    }
    fn cache_dir(&self) -> &Path {
        self.cache_dir
    }
}
