//! Owned I/O requests and input installation. Jobs never retain a database.
use super::*;
use lspkl_project::routing::{RoutingFacts, RoutingRequest};
/// A still-missing input observed by the active request; converted to a
/// [`PendingIo`] after the request computation, outside any query.
#[derive(PartialEq)]
enum Observed {
    Source(Uri<String>),
    Directory(Uri<String>),
    Routing(RoutingRequest),
}
thread_local! {
    // Request-local observation; the guard clears it even on Salsa cancellation.
    static OBSERVED: std::cell::RefCell<Option<Vec<Observed>>> = const { std::cell::RefCell::new(None) };
}
/// Record a still-missing input for the active request observation, if any.
/// `missing` returns `None` for inputs that are already installed.
fn observe(missing: impl FnOnce() -> Option<Observed>) {
    OBSERVED.with(|observed| {
        if let Some(jobs) = observed.borrow_mut().as_mut()
            && let Some(job) = missing()
            && !jobs.contains(&job)
        {
            jobs.push(job);
        }
    });
}
pub(crate) fn observe_source(db: &dyn Db, uri: &Uri<String>) {
    observe(|| {
        let pending = db.files().pending_sources.lock().contains_key(uri.as_str());
        pending.then(|| Observed::Source(uri.clone()))
    });
}
pub(crate) fn observe_directory(db: &dyn Db, root: &Uri<String>) {
    observe(|| {
        let pending = db
            .files()
            .pending_directories
            .lock()
            .contains_key(root.as_str());
        pending.then(|| Observed::Directory(root.clone()))
    });
}
fn observe_routing(db: &dyn Db, key: &str, request: &RoutingRequest) {
    observe(|| {
        let pending = db.files().pending_routing.lock().contains_key(key);
        pending.then(|| Observed::Routing(request.clone()))
    });
}
struct ObservationGuard;
impl Drop for ObservationGuard {
    fn drop(&mut self) {
        OBSERVED.with(|observed| {
            observed.borrow_mut().take();
        });
    }
}
#[derive(Debug, Clone, PartialEq)]
pub enum PendingIo {
    Source {
        uri: Uri<String>,
        cache_dir: PathBuf,
    },
    Directory(Uri<String>),
    Routing(RoutingRequest),
}
pub enum IoUpdate {
    Source {
        uri: Uri<String>,
        text: Option<Arc<str>>,
    },
    Directory(DirectoryUpdate),
    Routing {
        request: RoutingRequest,
        root: Option<Uri<String>>,
    },
}
impl PendingIo {
    /// Run on the owned I/O worker, without any Salsa handle.
    pub fn load(self, loader: &dyn SourceLoader, https: &StandardSourceLoader) -> IoUpdate {
        match self {
            Self::Source { uri, cache_dir } => {
                let source = loader.load(&uri, &cache_dir).or_else(|error| {
                    if uri.scheme().as_str() == "https" {
                        https.fetch(&uri, &cache_dir)
                    } else {
                        Err(error)
                    }
                });
                IoUpdate::Source {
                    uri,
                    text: source.ok().map(|source| source.text.clone()),
                }
            }
            Self::Directory(root) => IoUpdate::Directory(DirectoryUpdate::load(loader, &root)),
            Self::Routing(request) => {
                let root = request.load();
                IoUpdate::Routing { request, root }
            }
        }
    }
    /// The update a failed load installs, so the input stops being pending.
    pub fn failed(self) -> IoUpdate {
        match self {
            Self::Source { uri, .. } => IoUpdate::Source { uri, text: None },
            Self::Directory(root) => IoUpdate::Directory(DirectoryUpdate {
                root,
                members: Vec::new(),
            }),
            Self::Routing(request) => IoUpdate::Routing {
                request,
                root: None,
            },
        }
    }
}
impl RootDatabase {
    /// Run `work` and report the missing inputs it (or the root's import graph,
    /// whose resolutions may be memoized) depends on. Only still-pending inputs
    /// are reported, so an empty result means the request saw complete facts.
    pub fn discover_request_io<R>(
        &self,
        root: Option<File>,
        work: impl FnOnce() -> R,
    ) -> (R, Vec<PendingIo>) {
        OBSERVED.with(|observed| *observed.borrow_mut() = Some(Vec::new()));
        let _guard = ObservationGuard;
        let result = work();
        if let Some(root) = root {
            // Memoized resolutions do not re-run `Db::file` or routing lookups,
            // so replay the root graph's edges to observe them.
            let graph = import_graph(self, root);
            for &(file, owner) in graph.edges.keys() {
                replay_import(
                    self,
                    file,
                    owner,
                    graph.projects.get(&file).copied().flatten(),
                );
            }
        }
        let observed = OBSERVED.with(|observed| observed.borrow_mut().take().unwrap_or_default());
        let cache_dir = self.config().cache_dir(self);
        let jobs = observed
            .into_iter()
            .map(|observed| match observed {
                Observed::Source(uri) => PendingIo::Source {
                    uri,
                    cache_dir: cache_dir.clone(),
                },
                Observed::Directory(root) => PendingIo::Directory(root),
                Observed::Routing(request) => PendingIo::Routing(request),
            })
            .collect();
        (result, jobs)
    }

    /// Deterministically collect facts needed by open-document import graphs.
    /// The caller deduplicates in-flight jobs and validates its input epoch before applying.
    pub fn pending_io(&self) -> Vec<PendingIo> {
        for file in self.all_files() {
            if file.status(self) == FileStatus::Open {
                let _ = import_graph(self, file);
            }
        }
        let cache_dir = self.config().cache_dir(self).clone();
        let mut jobs = Vec::new();
        jobs.extend(
            self.files
                .pending_sources
                .lock()
                .values()
                .cloned()
                .map(|uri| PendingIo::Source {
                    uri,
                    cache_dir: cache_dir.clone(),
                }),
        );
        jobs.extend(
            self.files
                .pending_directories
                .lock()
                .values()
                .cloned()
                .map(PendingIo::Directory),
        );
        jobs.extend(
            self.files
                .pending_routing
                .lock()
                .values()
                .cloned()
                .map(PendingIo::Routing),
        );
        jobs
    }
    pub fn apply_io(&mut self, update: IoUpdate) -> Result<(), FileError> {
        match update {
            IoUpdate::Source { uri, text } => {
                self.apply_source(&uri, text)?;
            }
            IoUpdate::Directory(update) => {
                self.files
                    .pending_directories
                    .lock()
                    .remove(update.root.as_str());
                self.apply_directory_updates(vec![update]);
            }
            IoUpdate::Routing { request, root } => {
                let key = format!("{request:?}");
                let input = self.files.routing.lock().get(&key).map(|(_, input)| *input);
                if let Some(input) = input
                    && input.root(self) != &root
                {
                    input.set_root(self).to(root);
                }
                self.files.pending_routing.lock().remove(&key);
            }
        }
        Ok(())
    }
    /// Watched backing files and package downloads refresh previously observed routing facts.
    pub fn request_routing_refresh(&mut self) {
        for (key, (request, _)) in self.files.routing.lock().iter() {
            self.files
                .pending_routing
                .lock()
                .insert(key.clone(), request.clone());
        }
    }
}
pub(crate) struct DatabaseRoutingFacts<'db> {
    pub db: &'db dyn Db,
}
impl RoutingFacts for DatabaseRoutingFacts<'_> {
    fn root(&self, request: RoutingRequest) -> Option<Uri<String>> {
        let key = format!("{request:?}");
        let root = {
            let mut facts = self.db.files().routing.lock();
            let (_, input) = facts.entry(key.clone()).or_insert_with(|| {
                self.db
                    .files()
                    .pending_routing
                    .lock()
                    .insert(key.clone(), request.clone());
                (request.clone(), RoutingInput::new(self.db, None))
            });
            input.root(self.db).clone()
        };
        observe_routing(self.db, &key, &request);
        root
    }
}
