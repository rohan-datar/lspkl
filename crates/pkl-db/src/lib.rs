//! Incremental file, project, and import database. Source fetching is external to queries.

pub mod settings;

use ::line_index::LineIndex;
use fluent_uri::Uri;
use parking_lot::Mutex;
use pkl_project::{PklProject, import_resolver::ImportResolver};
use pkl_syntax::{
    ParseError,
    ast::{NodeId, SyntaxTree},
};
use pkl_vfs::{FileError, Source, SourceLoader, StandardSourceLoader, effective_uri, url_to_path};
use salsa::{Setter, plumbing::AsId};
use settings::WorkspaceSettings;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Present,
    Open,
    Missing,
}

#[salsa::input]
#[derive(Debug)]
pub struct File {
    #[returns(ref)]
    pub uri: Uri<String>,
    #[returns(ref)]
    pub text: Arc<str>,
    #[returns(copy)]
    pub version: Option<i32>,
    #[returns(copy)]
    pub status: FileStatus,
    #[returns(copy)]
    pub project: Option<Project>,
}
impl PartialOrd for File {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for File {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_id().cmp(&other.as_id())
    }
}

#[salsa::input]
pub struct WorkspaceConfig {
    #[returns(ref)]
    pub settings: WorkspaceSettings,
    #[returns(ref)]
    pub cache_dir: PathBuf,
    #[returns(ref)]
    pub modulepath: Vec<PathBuf>,
}
#[salsa::input]
#[derive(Debug)]
pub struct Project {
    #[returns(ref)]
    pub metadata: Option<Arc<PklProject>>,
}
#[salsa::input]
pub struct Directory {
    #[returns(ref)]
    pub members: Vec<Uri<String>>,
    /// Content fingerprints for routing metadata/cache inspection outside SourceLoader.
    #[returns(ref)]
    pub backing_state: Vec<(String, u64)>,
}

/// Canonical URI identities, shared by database snapshots. Never caches query results.
#[derive(Default)]
pub struct Files {
    entries: Mutex<BTreeMap<String, File>>,
    directories: Mutex<BTreeMap<String, Directory>>,
    projects: Mutex<BTreeMap<PathBuf, Project>>,
}

#[salsa::db]
pub trait Db: salsa::Database {
    fn files(&self) -> &Files;
    fn source_loader(&self) -> &dyn SourceLoader;
    fn config(&self) -> WorkspaceConfig;
    fn file(&self, uri: &Uri<String>) -> Result<File, FileError>;
    fn directory(&self, root: &Uri<String>) -> Result<Directory, FileError>;
}

#[salsa::db]
#[derive(Clone)]
pub struct RootDatabase {
    storage: salsa::Storage<Self>,
    files: Arc<Files>,
    loader: Arc<dyn SourceLoader + Send + Sync>,
    config: Option<WorkspaceConfig>,
}
#[salsa::db]
impl salsa::Database for RootDatabase {}
#[salsa::db]
impl Db for RootDatabase {
    fn files(&self) -> &Files {
        &self.files
    }
    fn source_loader(&self) -> &dyn SourceLoader {
        &*self.loader
    }
    fn config(&self) -> WorkspaceConfig {
        self.config.expect("database initialized")
    }
    fn file(&self, uri: &Uri<String>) -> Result<File, FileError> {
        let uri = effective_uri(uri)?;
        let mut entries = self.files.entries.lock();
        if let Some(file) = entries.get(uri.as_str()) {
            return Ok(*file);
        }
        let source = self.loader.load(&uri, self.config().cache_dir(self));
        let (text, status) = match source {
            Ok(source) => (source.text.clone(), FileStatus::Present),
            Err(_) => (Arc::from(""), FileStatus::Missing),
        };
        let project = self.local_project(&uri);
        let file = File::new(self, uri.clone(), text, None, status, project);
        entries.insert(uri.as_str().to_owned(), file);
        Ok(file)
    }
    fn directory(&self, root: &Uri<String>) -> Result<Directory, FileError> {
        let root = effective_uri(root)?;
        let mut directories = self.files.directories.lock();
        if let Some(directory) = directories.get(root.as_str()) {
            return Ok(*directory);
        }
        // A missing directory still has an input, so watched creation invalidates globs.
        let members = self.loader.list_files(&root).unwrap_or_default();
        let backing_state = backing_state(&root, &members);
        let directory = Directory::new(self, members, backing_state);
        directories.insert(root.as_str().to_owned(), directory);
        Ok(directory)
    }
}

impl Default for RootDatabase {
    fn default() -> Self {
        Self::new(
            Arc::new(StandardSourceLoader::default()),
            WorkspaceSettings::default(),
        )
    }
}
impl RootDatabase {
    pub fn new(loader: Arc<dyn SourceLoader + Send + Sync>, settings: WorkspaceSettings) -> Self {
        Self::with_event_callback(loader, settings, None)
    }
    pub fn with_event_callback(
        loader: Arc<dyn SourceLoader + Send + Sync>,
        settings: WorkspaceSettings,
        callback: Option<Box<dyn Fn(salsa::Event) + Send + Sync>>,
    ) -> Self {
        let mut db = Self {
            storage: salsa::Storage::new(callback),
            files: Arc::default(),
            loader,
            config: None,
        };
        db.config = Some(WorkspaceConfig::new(
            &db,
            settings.clone(),
            settings.effective_cache_dir().to_owned(),
            settings.module_paths.clone(),
        ));
        for name in pkl_stdlib::MODULE_NAMES {
            let uri = Uri::parse(format!("pkl:{name}")).expect("embedded URI");
            let text = Arc::from(pkl_stdlib::source(name).expect("embedded source"));
            let file = File::builder(uri.clone(), text, None, FileStatus::Present, None)
                .uri_durability(salsa::Durability::HIGH)
                .text_durability(salsa::Durability::HIGH)
                .new(&db);
            db.files
                .entries
                .lock()
                .insert(uri.as_str().to_owned(), file);
        }
        db
    }
    pub fn known_file(&self, uri: &Uri<String>) -> Option<File> {
        let uri = effective_uri(uri).ok()?;
        self.files.entries.lock().get(uri.as_str()).copied()
    }
    pub fn source_loader_handle(&self) -> Arc<dyn SourceLoader + Send + Sync> {
        self.loader.clone()
    }
    pub fn directory_roots(&self) -> Vec<Uri<String>> {
        self.files
            .directories
            .lock()
            .keys()
            .map(|root| Uri::parse(root.clone()).expect("directory URI"))
            .collect()
    }
    pub fn apply_directory_updates(&mut self, updates: Vec<DirectoryUpdate>) {
        for update in updates {
            let directory = self
                .files
                .directories
                .lock()
                .get(update.root.as_str())
                .copied();
            if let Some(directory) = directory {
                if directory.members(self) != &update.members {
                    directory.set_members(self).to(update.members);
                }
                if directory.backing_state(self) != &update.backing_state {
                    directory.set_backing_state(self).to(update.backing_state);
                }
            }
        }
    }
    /// Install an externally loaded source; an open editor buffer remains authoritative.
    pub fn apply_source(
        &mut self,
        uri: &Uri<String>,
        text: Option<Arc<str>>,
    ) -> Result<File, FileError> {
        if let Some(file) = self.known_file(uri)
            && (file.status(self) == FileStatus::Open || file.uri(self).scheme().as_str() == "pkl")
        {
            return Ok(file);
        }
        let status = if text.is_some() {
            FileStatus::Present
        } else {
            FileStatus::Missing
        };
        self.upsert_file(uri, text.unwrap_or_else(|| Arc::from("")), None, status)
    }
    pub fn begin_close(&mut self, uri: &Uri<String>) {
        if let Some(file) = self.known_file(uri) {
            file.set_status(self).to(FileStatus::Present);
            file.set_version(self).to(None);
        }
    }
    pub fn set_directory(
        &mut self,
        root: &Uri<String>,
        members: Vec<Uri<String>>,
    ) -> Result<(), FileError> {
        let root = effective_uri(root)?;
        let directory = self.files.directories.lock().get(root.as_str()).copied();
        if let Some(directory) = directory {
            if directory.members(self) != &members {
                directory.set_members(self).to(members);
            }
        } else {
            let directory = Directory::new(self, members, Vec::new());
            self.files
                .directories
                .lock()
                .insert(root.as_str().to_owned(), directory);
        }
        Ok(())
    }
    fn local_project(&self, uri: &Uri<String>) -> Option<Project> {
        let path = url_to_path(uri).ok()?;
        self.files
            .projects
            .lock()
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(_, project)| *project)
    }
    pub fn set_settings(&mut self, settings: WorkspaceSettings) {
        let config = self.config();
        let cache = settings.effective_cache_dir().to_owned();
        if config.cache_dir(self) != &cache {
            config.set_cache_dir(self).to(cache);
        }
        if config.modulepath(self) != &settings.module_paths {
            config
                .set_modulepath(self)
                .to(settings.module_paths.clone());
        }
        if config.settings(self) != &settings {
            config.set_settings(self).to(settings);
        }
    }
    /// Update individual metadata inputs, retaining project identities across refreshes.
    pub fn set_projects(&mut self, projects: Vec<Arc<PklProject>>) {
        let old = self.files.projects.lock().clone();
        let mut current = BTreeMap::new();
        for metadata in projects {
            let Some(root) = metadata.project_dir() else {
                continue;
            };
            let project = old
                .get(&root)
                .copied()
                .unwrap_or_else(|| Project::new(self, None));
            if project.metadata(self).as_ref() != Some(&metadata) {
                project.set_metadata(self).to(Some(metadata));
            }
            current.insert(root, project);
        }
        for (root, project) in old {
            if !current.contains_key(&root) {
                project.set_metadata(self).to(None);
            }
        }
        *self.files.projects.lock() = current;
        let files: Vec<_> = self.files.entries.lock().values().copied().collect();
        for file in files {
            let project = self.local_project(file.uri(self));
            if file.project(self) != project {
                file.set_project(self).to(project);
            }
        }
    }
    /// Create fixtures and editor buffers without synthesizing salsa identities.
    pub fn upsert_file(
        &mut self,
        uri: &Uri<String>,
        text: Arc<str>,
        version: Option<i32>,
        status: FileStatus,
    ) -> Result<File, FileError> {
        let uri = effective_uri(uri)?;
        let file = if let Some(file) = self.known_file(&uri) {
            file
        } else {
            let project = self.local_project(&uri);
            let file = File::new(self, uri.clone(), text.clone(), version, status, project);
            self.files
                .entries
                .lock()
                .insert(uri.as_str().to_owned(), file);
            file
        };
        if file.uri(self).scheme().as_str() != "pkl" && file.text(self) != &text {
            file.set_text(self).to(text);
        }
        if file.version(self) != version {
            file.set_version(self).to(version);
        }
        if file.status(self) != status {
            file.set_status(self).to(status);
        }
        Ok(file)
    }
    pub fn open_file(
        &mut self,
        uri: &Uri<String>,
        text: Arc<str>,
        version: Option<i32>,
    ) -> Result<File, FileError> {
        self.upsert_file(uri, text, version, FileStatus::Open)
    }
    pub fn refresh_file(&mut self, uri: &Uri<String>) -> Result<File, FileError> {
        let file = self.file(uri)?;
        if file.status(self) == FileStatus::Open || file.uri(self).scheme().as_str() == "pkl" {
            return Ok(file);
        }
        let source = self
            .loader
            .load(file.uri(self), self.config().cache_dir(self));
        let (text, status) = match source {
            Ok(source) => (source.text.clone(), FileStatus::Present),
            Err(_) => (Arc::from(""), FileStatus::Missing),
        };
        self.upsert_file(uri, text, None, status)
    }
    pub fn close_file(&mut self, uri: &Uri<String>) -> Result<File, FileError> {
        self.begin_close(uri);
        self.refresh_file(uri)
    }
    pub fn open_files(&self) -> Vec<File> {
        self.files
            .entries
            .lock()
            .values()
            .copied()
            .filter(|file| file.status(self) == FileStatus::Open)
            .collect()
    }
    pub fn all_files(&self) -> Vec<File> {
        self.files.entries.lock().values().copied().collect()
    }
    /// Reload observed directory inputs after watched creation/deletion or cache fetches.
    pub fn reload_directories(&mut self) {
        let directories = self.files.directories.lock().clone();
        for (root, directory) in directories {
            let root = Uri::parse(root).expect("canonical directory URI");
            let members = self.loader.list_files(&root).unwrap_or_default();
            let state = backing_state(&root, &members);
            if directory.backing_state(self) != &state {
                directory.set_backing_state(self).to(state);
            }
            if directory.members(self) != &members {
                directory.set_members(self).to(members);
            }
        }
    }
}

#[salsa::tracked(returns(ref), no_eq)]
pub fn parse(db: &dyn Db, file: File) -> Result<Arc<SyntaxTree>, Arc<ParseError>> {
    db.unwind_if_revision_cancelled();
    let result =
        pkl_syntax::parse_with_line_index(file.text(db).clone(), line_index(db, file).clone())
            .map(Arc::new)
            .map_err(Arc::new);
    db.unwind_if_revision_cancelled();
    result
}
#[salsa::tracked(returns(ref), no_eq)]
pub fn line_index(db: &dyn Db, file: File) -> Arc<LineIndex> {
    Arc::new(LineIndex::new(file.text(db)))
}
#[salsa::tracked(returns(copy))]
pub fn project_for_file(db: &dyn Db, file: File) -> Option<Project> {
    file.project(db)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportResolution {
    Single(File),
    Glob(Vec<File>),
    Unresolved { package_to_download: Option<String> },
}

/// Contextual edges for one request root; syntax remains in the parse query.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportGraph {
    pub edges: HashMap<(File, NodeId), ImportResolution>,
    pub projects: HashMap<File, Option<Project>>,
}

#[salsa::tracked(returns(ref))]
pub fn import_graph(db: &dyn Db, root: File) -> ImportGraph {
    fn visit(
        db: &dyn Db,
        file: File,
        context: Option<Project>,
        visited: &mut HashSet<File>,
        graph: &mut ImportGraph,
    ) {
        if !visited.insert(file) || file.status(db) == FileStatus::Missing {
            return;
        }
        let context = if file.uri(db).scheme().as_str() == "file" {
            project_for_file(db, file)
        } else {
            context
        };
        graph.projects.insert(file, context);
        let Ok(tree) = parse(db, file) else { return };
        fn walk(
            db: &dyn Db,
            file: File,
            tree: &SyntaxTree,
            node: NodeId,
            context: Option<Project>,
            visited: &mut HashSet<File>,
            graph: &mut ImportGraph,
        ) {
            if tree.kind(node).is_module_uri_owner() {
                let resolution = resolve_import(db, file, node, context);
                match &resolution {
                    ImportResolution::Single(target) => visit(db, *target, context, visited, graph),
                    ImportResolution::Glob(targets) => {
                        for target in targets {
                            visit(db, *target, context, visited, graph);
                        }
                    }
                    ImportResolution::Unresolved { .. } => {}
                }
                graph.edges.insert((file, node), resolution);
            }
            for child in tree.children(node) {
                walk(db, file, tree, *child, context, visited, graph);
            }
        }
        walk(db, file, tree, tree.root(), context, visited, graph);
    }
    let mut graph = ImportGraph::default();
    visit(db, root, None, &mut HashSet::new(), &mut graph);
    graph
}

/// Query-local overlay: every attempted source and directory read is a dependency.
pub struct DatabaseSourceLoader<'db> {
    db: &'db dyn Db,
}
impl<'db> DatabaseSourceLoader<'db> {
    pub fn new(db: &'db dyn Db) -> Self {
        Self { db }
    }
}
impl SourceLoader for DatabaseSourceLoader<'_> {
    fn load(&self, uri: &Uri<String>, _cache: &Path) -> Result<Arc<Source>, FileError> {
        let file = self.db.file(uri)?;
        if file.status(self.db) == FileStatus::Missing {
            return Err(FileError::InvalidSource(format!(
                "source unavailable: {uri}"
            )));
        }
        Ok(Arc::new(Source {
            url: file.uri(self.db).clone(),
            text: file.text(self.db).clone(),
        }))
    }
    fn list_files(&self, root: &Uri<String>) -> Result<Vec<Uri<String>>, FileError> {
        Ok(self.db.directory(root)?.members(self.db).clone())
    }
}

#[salsa::tracked(returns(clone))]
pub fn resolve_import(
    db: &dyn Db,
    file: File,
    owner: NodeId,
    context: Option<Project>,
) -> ImportResolution {
    let unresolved = || ImportResolution::Unresolved {
        package_to_download: None,
    };
    let Ok(tree) = parse(db, file) else {
        return unresolved();
    };
    let reference = if owner == NodeId::ROOT {
        tree.module_extends_or_amends_target()
    } else {
        use pkl_syntax::ast::nodes::{Import, ImportExpr, ImportGlob, ModuleExtendsAmendsClause};
        Import::cast(tree, owner)
            .and_then(|node| node.uri(tree))
            .or_else(|| ImportGlob::cast(tree, owner).and_then(|node| node.uri(tree)))
            .or_else(|| ImportExpr::cast(tree, owner).and_then(|node| node.uri(tree)))
            .or_else(|| {
                ModuleExtendsAmendsClause::cast(tree, owner).and_then(|node| node.target(tree))
            })
            .and_then(|string| tree.escaped_string_literal(string))
    };
    let Some(reference) = reference else {
        return unresolved();
    };
    resolve_module_uri(db, file, reference, tree.is_glob_import(owner), context)
}

/// Resolve a lowered URI without depending on the importing source tree.
#[salsa::tracked(returns(clone))]
pub fn resolve_module_uri(
    db: &dyn Db,
    file: File,
    reference: String,
    glob: bool,
    context: Option<Project>,
) -> ImportResolution {
    let unresolved = || ImportResolution::Unresolved {
        package_to_download: None,
    };
    let config = db.config();
    let loader = DatabaseSourceLoader::new(db);
    let resolver =
        ImportResolver::with_settings(&loader, config.modulepath(db), config.cache_dir(db));
    let project = if file.uri(db).scheme().as_str() == "file" {
        project_for_file(db, file)
    } else {
        context
    };
    let project = project.and_then(|project| project.metadata(db).as_deref());
    // Package availability and root classification inspect deterministic disk state.
    // Observe only roots relevant to this resolution; ordinary text edits leave these inputs alone.
    let mut roots = config.modulepath(db).clone();
    if let Some(project) = project {
        roots.extend(
            project
                .metadata
                .evaluator_settings
                .iter()
                .flat_map(|settings| settings.module_path.iter().flatten())
                .map(|path| project.project_dir().unwrap_or_default().join(path)),
        );
    }
    if reference.starts_with("package:")
        || reference.starts_with("@")
        || file.uri(db).scheme().as_str() == "jar"
    {
        roots.push(config.cache_dir(db).clone());
    }
    for root in roots {
        if let Ok(uri) = pkl_vfs::path_to_uri(&root)
            && let Ok(directory) = db.directory(&uri)
        {
            let _ = directory.backing_state(db);
        }
    }
    let source = Source {
        url: file.uri(db).clone(),
        text: Arc::from(""),
    };
    if glob {
        let files: Vec<_> = resolver
            .resolve_glob(&reference, &source, project)
            .iter()
            .filter_map(|source| db.file(&source.url).ok())
            .collect();
        if files.is_empty()
            && let Some(package) = resolver.missing_package_uri(&reference, &source, project)
        {
            ImportResolution::Unresolved {
                package_to_download: Some(package),
            }
        } else {
            ImportResolution::Glob(files)
        }
    } else if let Some(source) = resolver.resolve_file(&reference, &source, project) {
        db.file(&source.url)
            .map(ImportResolution::Single)
            .unwrap_or_else(|_| unresolved())
    } else {
        ImportResolution::Unresolved {
            package_to_download: resolver.missing_package_uri(&reference, &source, project),
        }
    }
}

/// Scalar projection shields semantic queries from source-only graph changes.
#[salsa::tracked(returns(copy))]
pub fn contextual_project(db: &dyn Db, root: File, file: File) -> Option<Project> {
    import_graph(db, root)
        .projects
        .get(&file)
        .copied()
        .flatten()
}

fn backing_state(root: &Uri<String>, members: &[Uri<String>]) -> Vec<(String, u64)> {
    use std::{
        hash::{Hash, Hasher},
        io::Read,
    };
    let mut result = Vec::new();
    for uri in std::iter::once(root).chain(members) {
        // Module text has its own File input; only routing artifacts need fingerprints.
        if uri != root && uri.path().as_str().ends_with(".pkl") {
            continue;
        }
        let Ok(path) = url_to_path(uri) else {
            continue;
        };
        let Ok(mut file) = std::fs::File::open(path) else {
            continue;
        };
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        let mut buffer = [0u8; 8192];
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(length) => buffer[..length].hash(&mut hash),
                Err(_) => break,
            }
        }
        result.push((uri.as_str().to_owned(), hash.finish()));
    }
    result
}

pub struct DirectoryUpdate {
    root: Uri<String>,
    members: Vec<Uri<String>>,
    backing_state: Vec<(String, u64)>,
}
impl DirectoryUpdate {
    pub fn load(loader: &dyn SourceLoader, root: &Uri<String>) -> Self {
        let members = loader.list_files(root).unwrap_or_default();
        let backing_state = backing_state(root, &members);
        Self {
            root: root.clone(),
            members,
            backing_state,
        }
    }
}
