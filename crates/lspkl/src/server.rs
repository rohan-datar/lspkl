//! Ordered input ownership, demand-dispatched computation, and owned external jobs.
use crate::{
    handlers::{self, HandlerError},
    perf,
    state::{LoadedFiles, ReloadWork, Snapshot, Workspace},
};
use crossbeam_channel::{Sender, bounded, select_biased, unbounded};
use lsp_server::{
    Connection, Message, Notification, ReqQueue, Request, RequestId, Response, ResponseError,
};
use lsp_types::*;
use pkl_db::settings::WorkspaceSettings;
use pkl_project::project_manager::ProjectManager;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

/// Owned work for an I/O worker. `panicked` builds the completion delivered when
/// `run` unwinds, so request registration and input bookkeeping are always cleared.
struct IoJob {
    run: Box<dyn FnOnce() -> Event + Send>,
    panicked: Box<dyn FnOnce() -> Event + Send>,
}
impl IoJob {
    fn new(
        run: impl FnOnce() -> Event + Send + 'static,
        panicked: impl FnOnce() -> Event + Send + 'static,
    ) -> Self {
        Self {
            run: Box::new(run),
            panicked: Box::new(panicked),
        }
    }
    fn request(id: RequestId, run: impl FnOnce() -> Event + Send + 'static) -> Self {
        Self::new(run, move || {
            Event::External(
                id,
                Err(HandlerError::Domain("external job panicked".into())),
            )
        })
    }
    fn execute(self) -> Event {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(self.run))
            .unwrap_or_else(|_| (self.panicked)())
    }
}
type Result<T, E = HandlerError> = std::result::Result<T, E>;
enum Effect {
    Value(Value),
    Source(handlers::documents::FileContentsResult),
    Projects(Option<ProjectManager>),
}
enum Prepared {
    Done(Effect),
    Io(IoJob),
    Retry(Request, Vec<pkl_db::PendingIo>, Effect),
    /// An input write cancelled the computation; the request itself is intact.
    Superseded(Request),
}
enum Event {
    Ready,
    Startup(u64, u64, WorkspaceSettings, Result<ProjectManager>),
    Computed(RequestId, Result<Prepared>),
    External(RequestId, Result<Effect>),
    Diagnostics(
        Uri,
        u64,
        Option<i32>,
        Result<(Vec<Diagnostic>, Vec<pkl_db::PendingIo>)>,
    ),
    Reload(Option<LoadedFiles>),
    Restore(u64, Result<ProjectManager>),
    Settings(u64, WorkspaceSettings),
    PendingIo(u64, Result<Vec<pkl_db::PendingIo>>),
    LoadedIo(u64, pkl_db::PendingIo, pkl_db::IoUpdate),
}
struct Pending {
    request: Request,
    span: tracing::Span,
    queued: std::time::Instant,
    document: Option<(String, u64)>,
    waiting: Vec<pkl_db::PendingIo>,
    fallback: Option<Effect>,
    retries: u8,
    deadline: std::time::Instant,
    handler: perf::Handler,
}
enum Outgoing {
    Configuration(u64),
    Registration,
}
struct Running {
    document: Option<(String, u64)>,
    analysis: u64,
    projects: u64,
    project_job: bool,
    retries: u8,
    deadline: std::time::Instant,
    span: tracing::Span,
    _handler: perf::Handler,
}
/// Recomputations per request: input waits plus cancellations by background writes.
const MAX_RETRIES: u8 = 8;
/// How long a request waits for missing inputs before answering with what it has.
const INPUT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
enum ComputeJob {
    Request(Snapshot, Request, tracing::Span, std::time::Instant),
    Diagnostics(Snapshot, Uri, u64, Option<i32>),
    PendingIo(Snapshot, u64),
}

fn params<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|e| HandlerError::InvalidParams(e.to_string()))
}
fn value<T: Serialize>(value: T) -> Result<Prepared> {
    Ok(Prepared::Done(Effect::Value(
        serde_json::to_value(value).expect("protocol result"),
    )))
}
fn prepare(snapshot: &Snapshot, request: Request) -> Result<Prepared> {
    use handlers::{commands as c, documents as d};
    let id = request.id;
    let p = request.params;
    match request.method.as_str() {
        "textDocument/hover" => value(d::hover(snapshot, params(p)?)?),
        "textDocument/definition" => value(d::definition(snapshot, params(p)?)?),
        "textDocument/completion" => value(d::completion(snapshot, params(p)?)?),
        "completionItem/resolve" => value(d::resolve(snapshot, params(p)?)?),
        "textDocument/codeAction" => value(d::code_action(snapshot, params(p)?)?),
        "textDocument/formatting" => {
            let Some(job) = d::prepare_formatting(snapshot, params(p)?)? else {
                return value(Option::<Vec<TextEdit>>::None);
            };
            Ok(Prepared::Io(IoJob::request(id.clone(), move || {
                Event::External(
                    id,
                    d::formatting(job).map(|v| Effect::Value(serde_json::to_value(v).unwrap())),
                )
            })))
        }
        "pkl/fileContents" => {
            let job = d::prepare_file_contents(snapshot, params(p)?)?;
            Ok(Prepared::Io(IoJob::request(id.clone(), move || {
                Event::External(id, d::file_contents(job).map(Effect::Source))
            })))
        }
        "pkl/syncProjects" | "pkl/downloadPackage" | "workspace/executeCommand" => {
            let job = match request.method.as_str() {
                "pkl/syncProjects" => c::prepare_sync_projects(snapshot),
                "pkl/downloadPackage" => c::prepare_download_package(snapshot, params(p)?)?,
                _ => c::prepare_execute_command(snapshot, params(p)?)?,
            };
            Ok(Prepared::Io(IoJob::request(id.clone(), move || {
                Event::External(id, c::execute_command(job).map(Effect::Projects))
            })))
        }
        _ => unreachable!("only supported requests are queued"),
    }
}
fn error(id: RequestId, code: i32, message: impl Into<String>) -> Response {
    Response {
        id,
        response_result: Err(ResponseError {
            code,
            message: message.into(),
            data: None,
        }),
    }
}
fn handler_response(id: RequestId, result: Result<Value>) -> Response {
    match result {
        Ok(v) => Response::new_ok(id, v),
        Err(e) => error(
            id,
            match e {
                HandlerError::ContentModified => -32801,
                HandlerError::InvalidParams(_) => -32602,
                HandlerError::Domain(_) => -32603,
            },
            e.to_string(),
        ),
    }
}

/// Run a connection with a workspace owned exclusively by this thread.
/// External workers never receive database handles; queued compute retains params only.
pub fn run(
    connection: Connection,
    mut workspace: Workspace,
) -> std::result::Result<bool, Box<dyn std::error::Error>> {
    let (id, initialize) = loop {
        let (id, params) = connection.initialize_start()?;
        match serde_json::from_value::<InitializeParams>(params) {
            Ok(initialize) => break (id, initialize),
            Err(error) => connection.sender.send(Message::Response(self::error(
                id,
                -32602,
                error.to_string(),
            )))?,
        }
    };
    let capabilities = handlers::lifecycle::initialize(&mut workspace, &initialize);
    connection.initialize_finish(id, serde_json::to_value(capabilities)?)?;
    let (events_tx, events) = unbounded();
    let (compute_tx, compute) = bounded::<ComputeJob>(0);
    let tx = events_tx.clone();
    std::thread::spawn(move || {
        loop {
            if tx.send(Event::Ready).is_err() {
                break;
            }
            let Ok(job) = compute.recv() else { break };
            let event = match job {
                ComputeJob::PendingIo(snapshot, epoch) => {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        snapshot.run("pending_io", |s| Ok(s.db.pending_io()))
                    }))
                    .unwrap_or_else(|_| {
                        Err(HandlerError::Domain("compute worker panicked".into()))
                    });
                    Event::PendingIo(epoch, result)
                }
                ComputeJob::Request(snapshot, request, span, queued) => {
                    let _entered = span.enter();
                    let _compute = perf::Compute::queued("request", queued);
                    let id = request.id.clone();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        snapshot.run("request", |s| {
                            let root = request
                                .params
                                .pointer("/textDocument/uri")
                                .and_then(Value::as_str)
                                .and_then(|uri| pkl_vfs::parse_source_uri(uri).ok())
                                .and_then(|uri| s.db.known_file(&uri));
                            let (prepared, jobs) =
                                s.db.discover_request_io(root, || prepare(s, request.clone()));
                            match prepared? {
                                Prepared::Done(effect) if !jobs.is_empty() => {
                                    Ok(Prepared::Retry(request.clone(), jobs, effect))
                                }
                                prepared => Ok(prepared),
                            }
                        })
                    }))
                    .unwrap_or_else(|_| {
                        Err(HandlerError::Domain("compute worker panicked".into()))
                    });
                    let result = match result {
                        Err(HandlerError::ContentModified) => Ok(Prepared::Superseded(request)),
                        result => result,
                    };
                    let result = result.map(|prepared| match prepared {
                        Prepared::Io(IoJob { run, panicked }) => {
                            let span = span.clone();
                            let queued = std::time::Instant::now();
                            Prepared::Io(IoJob {
                                run: Box::new(move || {
                                    let _entered = span.enter();
                                    let _compute = perf::Compute::queued("external", queued);
                                    run()
                                }),
                                panicked,
                            })
                        }
                        prepared => prepared,
                    });
                    Event::Computed(id, result)
                }
                ComputeJob::Diagnostics(snapshot, uri, generation, version) => {
                    let _compute = perf::Compute::start("diagnostics");
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        snapshot.run("diagnostics", |s| {
                            let (diagnostics, jobs) =
                                s.db.discover_request_io(s.document(uri.as_str()), || {
                                    handlers::diagnostics::diagnostics(s, uri.as_str())
                                });
                            diagnostics.map(|diagnostics| (diagnostics, jobs))
                        })
                    }))
                    .unwrap_or_else(|_| {
                        Err(HandlerError::Domain("diagnostic worker panicked".into()))
                    });
                    Event::Diagnostics(uri, generation, version, result)
                }
            };
            if tx.send(event).is_err() {
                break;
            }
        }
    });
    let (io_tx, io) = unbounded::<IoJob>();
    for _ in 0..4 {
        let rx = io.clone();
        let tx = events_tx.clone();
        std::thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                if tx.send(job.execute()).is_err() {
                    break;
                }
            }
        });
    }
    // Persistence is serialized independently of external jobs, and only accepted
    // results are submitted. A rejected or cancelled job cannot touch this queue.
    let (persist_tx, persist) = unbounded::<ProjectManager>();
    let persist_thread = std::thread::spawn(move || {
        while let Ok(manager) = persist.recv() {
            for root in manager.workspace_folders() {
                if let Err(e) = manager.persist_state(&root.join(".lspkl/projects.json")) {
                    tracing::error!(?e, "project persistence failed");
                }
            }
        }
    });
    let _persist_thread = PersistenceThread(Some(persist_thread));
    let mut server = Loop {
        connection,
        workspace,
        queue: ReqQueue::default(),
        pending: VecDeque::new(),
        running: HashMap::new(),
        documents: HashMap::new(),
        dirty: HashMap::new(),
        io: io_tx,
        persist: persist_tx,
        configuration: 0,
        shutdown: false,
        started: false,
        reloads: 0,
        io_epoch: 0,
        collect_io: true,
        in_flight_io: Vec::new(),
        diagnostics_waiting: HashMap::new(),
        diagnostics_overdue: HashSet::new(),
    };
    let settings = server.workspace.settings().clone();
    let manager = server.workspace.project_manager();
    let folders = manager.workspace_folders().to_vec();
    let epoch = server.workspace.project_generation();
    let unresolved = settings.clone();
    let _ = server.io.send(IoJob::new(
        move || {
            let mut settings = settings;
            settings.resolve_paths(&folders);
            Event::Startup(
                epoch,
                0,
                settings,
                handlers::lifecycle::restore_workspaces(manager),
            )
        },
        move || {
            Event::Startup(
                epoch,
                0,
                unresolved,
                Err(HandlerError::Domain("startup job panicked".into())),
            )
        },
    ));
    let mut idle = false;
    loop {
        // Drain protocol traffic first, so control messages never sit behind work.
        while let Ok(message) = server.connection.receiver.try_recv() {
            if let Some(exit) = server.message(message) {
                return Ok(exit);
            }
        }
        server.expire_waiting();
        if idle && server.started && !server.shutdown {
            if server.collect_io {
                server.collect_io = false;
                compute_tx.send(ComputeJob::PendingIo(
                    server.workspace.snapshot(),
                    server.io_epoch,
                ))?;
                idle = false;
            }
            while idle && let Some(index) = server.pending.iter().position(|p| p.waiting.is_empty())
            {
                let pending = server.pending.remove(index).unwrap();
                if server.queue.incoming.is_completed(&pending.request.id) {
                    continue;
                }
                if server.document_changed(&pending.document) {
                    server.respond(pending.request.id, Err(HandlerError::ContentModified));
                    continue;
                }
                let request = pending.request;
                server.running.insert(
                    request.id.clone(),
                    Running {
                        document: pending.document,
                        retries: pending.retries,
                        deadline: pending.deadline,
                        span: pending.span.clone(),
                        _handler: pending.handler,
                        analysis: server.workspace.generation(),
                        projects: server.workspace.project_generation(),
                        project_job: matches!(
                            request.method.as_str(),
                            "pkl/syncProjects" | "pkl/downloadPackage" | "workspace/executeCommand"
                        ),
                    },
                );
                compute_tx.send(ComputeJob::Request(
                    server.workspace.snapshot(),
                    request,
                    pending.span,
                    pending.queued,
                ))?;
                idle = false;
                break;
            }
            if idle
                && server.reloads == 0
                && let Some((uri, generation)) =
                    server.dirty.iter().next().map(|(u, g)| (u.clone(), *g))
            {
                server.dirty.remove(&uri);
                let version = server.workspace.document_version(uri.as_str());
                compute_tx.send(ComputeJob::Diagnostics(
                    server.workspace.snapshot(),
                    uri,
                    generation,
                    version,
                ))?;
                idle = false;
            }
        }
        let deadline = server
            .pending
            .iter()
            .filter(|p| p.fallback.is_some())
            .map(|p| p.deadline)
            .chain(server.diagnostics_waiting.values().copied())
            .min();
        let timer = deadline.map_or_else(crossbeam_channel::never, |deadline| {
            crossbeam_channel::after(deadline.saturating_duration_since(std::time::Instant::now()))
        });
        select_biased! {
            recv(server.connection.receiver) -> message => {
                let Ok(message) = message else { return Ok(false) };
                if let Some(exit) = server.message(message) { return Ok(exit) }
            }
            recv(events) -> event => {
                match event? { Event::Ready => idle = true, event => server.event(event) }
            }
            recv(timer) -> _ => {}
        }
    }
}
struct PersistenceThread(Option<std::thread::JoinHandle<()>>);
impl Drop for PersistenceThread {
    fn drop(&mut self) {
        if let Some(thread) = self.0.take() {
            let _ = thread.join();
        }
    }
}

struct Loop {
    connection: Connection,
    workspace: Workspace,
    queue: ReqQueue<(), Outgoing>,
    pending: VecDeque<Pending>,
    running: HashMap<RequestId, Running>,
    documents: HashMap<String, u64>,
    dirty: HashMap<Uri, u64>,
    io: Sender<IoJob>,
    persist: Sender<ProjectManager>,
    configuration: u64,
    shutdown: bool,
    started: bool,
    reloads: usize,
    io_epoch: u64,
    collect_io: bool,
    in_flight_io: Vec<(u64, pkl_db::PendingIo)>,
    /// Documents whose diagnostics wait for their own missing inputs, until the deadline.
    diagnostics_waiting: HashMap<Uri, std::time::Instant>,
    /// Documents whose wait expired: publish the next result even if inputs are missing.
    diagnostics_overdue: HashSet<Uri>,
}
impl Loop {
    fn expire_waiting(&mut self) {
        let now = std::time::Instant::now();
        while let Some(index) = self
            .pending
            .iter()
            .position(|p| p.fallback.is_some() && p.deadline <= now)
        {
            let mut pending = self.pending.remove(index).unwrap();
            let id = pending.request.id;
            self.running.insert(
                id.clone(),
                Running {
                    document: pending.document,
                    analysis: self.workspace.generation(),
                    projects: self.workspace.project_generation(),
                    project_job: false,
                    retries: pending.retries,
                    deadline: pending.deadline,
                    span: pending.span,
                    _handler: pending.handler,
                },
            );
            self.finish(id, Ok(pending.fallback.take().unwrap()));
        }
        let expired: Vec<_> = self
            .diagnostics_waiting
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .map(|(uri, _)| uri.clone())
            .collect();
        for uri in expired {
            self.diagnostics_waiting.remove(&uri);
            self.dirty.insert(uri.clone(), self.workspace.generation());
            self.diagnostics_overdue.insert(uri);
        }
    }
    /// Whether the request's document was edited, closed, or reopened since arrival.
    fn document_changed(&self, document: &Option<(String, u64)>) -> bool {
        document
            .as_ref()
            .is_some_and(|(uri, epoch)| self.documents.get(uri).copied().unwrap_or(0) != *epoch)
    }
    /// Return a running request to the queue for another computation.
    fn requeue(
        &mut self,
        id: &RequestId,
        request: Request,
        waiting: Vec<pkl_db::PendingIo>,
        fallback: Option<Effect>,
    ) {
        let Some(running) = self.running.remove(id) else {
            return;
        };
        self.pending.push_back(Pending {
            request,
            document: running.document,
            handler: running._handler,
            span: running.span,
            queued: std::time::Instant::now(),
            waiting,
            fallback,
            retries: running.retries + 1,
            deadline: running.deadline,
        });
    }
    fn load_inputs(&mut self, jobs: Vec<pkl_db::PendingIo>) {
        let epoch = self.io_epoch;
        for job in jobs {
            // A load from an older epoch will be discarded, so it cannot satisfy this one.
            if self
                .in_flight_io
                .iter()
                .any(|(running, existing)| *running == epoch && existing == &job)
            {
                continue;
            }
            let (loader, https) = self.workspace.semantic_loaders();
            self.in_flight_io.push((epoch, job.clone()));
            let failed = job.clone();
            let _ = self.io.send(IoJob::new(
                move || {
                    let update = job.clone().load(loader.as_ref(), &https);
                    Event::LoadedIo(epoch, job, update)
                },
                move || Event::LoadedIo(epoch, failed.clone(), failed.failed()),
            ));
        }
    }
    fn send(&self, message: impl Into<Message>) {
        let _ = self.connection.sender.send(message.into());
    }
    fn respond(&mut self, id: RequestId, result: Result<Value>) {
        self.running.remove(&id);
        if self.queue.incoming.complete(&id).is_some() {
            self.send(handler_response(id, result));
        }
    }
    /// After any input write: rediscover missing inputs and refresh diagnostics.
    fn post_write(&mut self) {
        self.collect_io = true;
        self.mark_dirty();
    }
    /// After a write that changes what owned loads would return (settings,
    /// project metadata, watched files): results of in-flight loads are stale.
    /// Editor buffer edits do not, so typing never discards input loads.
    fn invalidate_io(&mut self) {
        self.io_epoch += 1;
        self.post_write();
    }
    fn mark_dirty(&mut self) {
        for uri in self.workspace.open_document_uris() {
            self.dirty.insert(uri, self.workspace.generation());
        }
    }
    fn reload(&mut self, work: ReloadWork) {
        self.reloads += 1;
        let _ = self.io.send(IoJob::new(
            move || Event::Reload(Some(work.load())),
            || Event::Reload(None),
        ));
    }
    fn restore(&self) {
        let epoch = self.workspace.project_generation();
        let manager = self.workspace.project_manager();
        let _ = self.io.send(IoJob::new(
            move || Event::Restore(epoch, handlers::lifecycle::restore_workspaces(manager)),
            move || {
                Event::Restore(
                    epoch,
                    Err(HandlerError::Domain("project restoration panicked".into())),
                )
            },
        ));
    }
    fn resolve_settings(&self) {
        let epoch = self.configuration;
        let settings = self.workspace.settings().clone();
        let unresolved = settings.clone();
        let folders = self
            .workspace
            .project_manager()
            .workspace_folders()
            .to_vec();
        // A panicked resolution reports the unresolved value, which equals the
        // installed settings and is therefore a no-op on the main thread.
        let _ = self.io.send(IoJob::new(
            move || {
                let mut settings = settings;
                settings.resolve_paths(&folders);
                Event::Settings(epoch, settings)
            },
            move || Event::Settings(epoch, unresolved),
        ));
    }
    fn pull_configuration(&mut self) {
        if !self.workspace.client_configuration() {
            return;
        }
        self.configuration += 1;
        let items: Vec<_> = [
            "pkl.cli.path",
            "pkl.formatter.grammarVersion",
            "pkl.modulepath",
            "pkl.projects.excludedDirectories",
            "pkl.cacheDir",
        ]
        .into_iter()
        .map(|s| json!({"section": s}))
        .collect();
        let request = self.queue.outgoing.register(
            "workspace/configuration".into(),
            json!({"items": items}),
            Outgoing::Configuration(self.configuration),
        );
        self.send(request);
    }
    fn register_watchers(&mut self) {
        if !self.workspace.watched_files {
            return;
        }
        let watchers: Vec<_> = ["**/*.pkl", "**/PklProject", "**/PklProject.deps.json"]
            .into_iter()
            .map(|p| json!({"globPattern":p,"kind":7}))
            .collect();
        let request = self.queue.outgoing.register("client/registerCapability".into(), json!({"registrations":[{"id":"pkl-watched-files","method":"workspace/didChangeWatchedFiles","registerOptions":{"watchers":watchers}}]}), Outgoing::Registration);
        self.send(request);
    }
    fn message(&mut self, message: Message) -> Option<bool> {
        let span = tracing::debug_span!(target: "lspkl::perf", "handler", handler_id = tracing::field::Empty, method = match &message { Message::Request(r) => r.method.as_str(), Message::Notification(n) => n.method.as_str(), Message::Response(_) => "response" });
        let _entered = span.enter();
        let _handler = perf::Handler::enter();
        match message {
            Message::Request(request) => {
                if self.shutdown {
                    self.send(error(request.id, -32600, "server is shutting down"));
                    return None;
                }
                if request.method == "shutdown" {
                    self.shutdown = true;
                    let ids: Vec<_> = self
                        .pending
                        .iter()
                        .map(|p| p.request.id.clone())
                        .chain(self.running.keys().cloned())
                        .collect();
                    for id in ids {
                        if let Some(response) = self.queue.incoming.cancel(id) {
                            self.send(response);
                        }
                    }
                    self.pending.clear();
                    self.send(Response::new_ok(request.id, Value::Null));
                    return None;
                }
                if !matches!(
                    request.method.as_str(),
                    "textDocument/hover"
                        | "textDocument/definition"
                        | "textDocument/completion"
                        | "completionItem/resolve"
                        | "textDocument/codeAction"
                        | "textDocument/formatting"
                        | "pkl/fileContents"
                        | "pkl/syncProjects"
                        | "pkl/downloadPackage"
                        | "workspace/executeCommand"
                ) {
                    self.send(error(request.id, -32601, "method not found"));
                    return None;
                }
                self.queue.incoming.register(request.id.clone(), ());
                let document = request
                    .params
                    .pointer("/textDocument/uri")
                    .and_then(Value::as_str)
                    .map(|u| (u.to_owned(), self.documents.get(u).copied().unwrap_or(0)));
                self.pending.push_back(Pending {
                    request,
                    document,
                    span: tracing::Span::current(),
                    queued: std::time::Instant::now(),
                    waiting: Vec::new(),
                    fallback: None,
                    retries: 0,
                    deadline: std::time::Instant::now() + INPUT_WAIT,
                    handler: _handler,
                });
            }
            Message::Notification(notification) => {
                if notification.method == "exit" {
                    return Some(self.shutdown);
                }
                if notification.method == "$/cancelRequest" {
                    if let Some(id) = notification
                        .params
                        .get("id")
                        .cloned()
                        .and_then(|v| serde_json::from_value::<RequestId>(v).ok())
                    {
                        if let Some(response) = self.queue.incoming.cancel(id.clone()) {
                            self.send(response);
                        }
                        self.pending.retain(|p| p.request.id != id);
                        self.running.remove(&id);
                    }
                    return None;
                }
                if self.shutdown {
                    return None;
                }
                if let Err(e) = self.notification(notification) {
                    tracing::error!(?e, "notification failed");
                }
            }
            Message::Response(response) => {
                if let Some(Outgoing::Configuration(epoch)) =
                    self.queue.outgoing.complete(response.id)
                    && epoch == self.configuration
                    && let Ok(Value::Array(values)) = response.response_result
                    && values.len() == 5
                {
                    let values: [Value; 5] = values.try_into().unwrap();
                    if self.workspace.apply_pulled_settings(&values).is_ok() {
                        self.resolve_settings();
                        self.invalidate_io();
                    }
                }
            }
        }
        None
    }
    fn notification(&mut self, n: Notification) -> Result<()> {
        use handlers::lifecycle as l;
        match n.method.as_str() {
            "textDocument/didOpen" => {
                let p: DidOpenTextDocumentParams = params(n.params)?;
                *self
                    .documents
                    .entry(p.text_document.uri.to_string())
                    .or_default() += 1;
                l::did_open(&mut self.workspace, p)?;
            }
            "textDocument/didChange" => {
                let p: DidChangeTextDocumentParams = params(n.params)?;
                *self
                    .documents
                    .entry(p.text_document.uri.to_string())
                    .or_default() += 1;
                l::did_change(&mut self.workspace, p)?;
            }
            "textDocument/didClose" => {
                let p: DidCloseTextDocumentParams = params(n.params)?;
                *self
                    .documents
                    .entry(p.text_document.uri.to_string())
                    .or_default() += 1;
                self.dirty.remove(&p.text_document.uri);
                self.diagnostics_waiting.remove(&p.text_document.uri);
                self.diagnostics_overdue.remove(&p.text_document.uri);
                let work = l::did_close(&mut self.workspace, p)?;
                self.reload(work);
            }
            "workspace/didChangeConfiguration" => {
                self.configuration += 1;
                let p: DidChangeConfigurationParams = params(n.params)?;
                if p.settings.is_null() || p.settings.as_object().is_some_and(|o| o.is_empty()) {
                    self.pull_configuration();
                } else {
                    l::did_change_configuration(&mut self.workspace, p)?;
                    self.resolve_settings();
                }
                self.invalidate_io();
                return Ok(());
            }
            "workspace/didChangeWorkspaceFolders" => {
                self.configuration += 1;
                l::did_change_workspace_folders(&mut self.workspace, params(n.params)?);
                self.resolve_settings();
                self.restore();
                self.invalidate_io();
                return Ok(());
            }
            "workspace/didChangeWatchedFiles" => {
                self.workspace.request_routing_refresh();
                let work = self.workspace.prepare_watched_files(params(n.params)?);
                self.workspace.invalidate_analysis();
                self.reload(work);
                self.invalidate_io();
                return Ok(());
            }
            _ => return Ok(()),
        }
        self.post_write();
        Ok(())
    }
    fn event(&mut self, event: Event) {
        if matches!(&event, Event::Computed(..) | Event::Diagnostics(..)) {
            self.collect_io = true;
        }
        match event {
            Event::PendingIo(epoch, result) => {
                if epoch != self.io_epoch {
                    self.collect_io = true;
                    return;
                }
                if let Ok(jobs) = result {
                    self.load_inputs(jobs);
                } else if matches!(result, Err(HandlerError::ContentModified)) {
                    self.collect_io = true;
                } else if let Err(error) = result {
                    tracing::error!(?error, "semantic input discovery failed");
                }
            }
            Event::LoadedIo(epoch, job, update) => {
                for pending in &mut self.pending {
                    pending.waiting.retain(|waiting| waiting != &job);
                }
                self.in_flight_io
                    .retain(|(running_epoch, running)| *running_epoch != epoch || running != &job);
                if epoch == self.io_epoch {
                    if let Err(error) = self.workspace.apply_io(update) {
                        tracing::error!(?error, "semantic input installation failed");
                    }
                    self.mark_dirty();
                }
                self.collect_io = true;
            }
            Event::Startup(project_epoch, configuration_epoch, settings, manager) => {
                if project_epoch == self.workspace.project_generation()
                    && let Ok(manager) = manager
                {
                    self.workspace.apply_project_manager(manager);
                }
                if configuration_epoch == self.configuration
                    && self.workspace.settings() != &settings
                {
                    self.workspace.apply_settings(settings);
                }
                self.started = true;
                self.register_watchers();
                self.pull_configuration();
                self.invalidate_io();
            }
            Event::Computed(id, result) => {
                if self.queue.incoming.is_completed(&id) {
                    self.running.remove(&id);
                    return;
                }
                match result {
                    Ok(Prepared::Retry(request, jobs, fallback)) => {
                        if self.running.get(&id).is_some_and(|r| {
                            r.retries >= MAX_RETRIES || r.deadline <= std::time::Instant::now()
                        }) {
                            self.finish(id, Ok(fallback));
                        } else {
                            self.load_inputs(jobs.clone());
                            self.requeue(&id, request, jobs, Some(fallback));
                        }
                    }
                    // Background input writes (loads, reloads) cancel computation without
                    // changing the request's document: compute it again rather than
                    // reporting a modification the client never made.
                    Ok(Prepared::Superseded(request)) => {
                        if self.running.get(&id).is_some_and(|r| {
                            r.retries < MAX_RETRIES && !self.document_changed(&r.document)
                        }) {
                            self.requeue(&id, request, Vec::new(), None);
                        } else {
                            self.respond(id, Err(HandlerError::ContentModified));
                        }
                    }
                    Ok(Prepared::Io(job)) => {
                        let _ = self.io.send(job);
                    }
                    Ok(Prepared::Done(effect)) => self.finish(id, Ok(effect)),
                    Err(e) => self.respond(id, Err(e)),
                }
            }
            Event::External(id, result) => self.finish(id, result),
            Event::Diagnostics(uri, generation, version, Ok((diagnostics, jobs)))
                if self.workspace.is_document_open(&uri)
                    && self.workspace.generation() == generation
                    && self.workspace.document_version(uri.as_str()) == version =>
            {
                // Hold back diagnostics computed without the document's own
                // imports until they load, so a new import does not flash as
                // unresolved; after `INPUT_WAIT` publish what is known.
                if !jobs.is_empty() && !self.diagnostics_overdue.contains(&uri) {
                    self.diagnostics_waiting
                        .entry(uri)
                        .or_insert_with(|| std::time::Instant::now() + INPUT_WAIT);
                    self.load_inputs(jobs);
                    return;
                }
                self.diagnostics_waiting.remove(&uri);
                self.diagnostics_overdue.remove(&uri);
                self.send(Notification::new(
                    "textDocument/publishDiagnostics".into(),
                    PublishDiagnosticsParams::new(uri, diagnostics, version),
                ));
            }
            Event::Diagnostics(_, _, _, Err(error))
                if !matches!(error, HandlerError::ContentModified) =>
            {
                tracing::error!(?error, "diagnostic analysis failed");
            }
            Event::Reload(loaded) => {
                self.reloads -= 1;
                match loaded {
                    Some(loaded) => {
                        self.workspace.apply_reload(loaded);
                        self.post_write();
                    }
                    None => tracing::error!("source reload panicked"),
                }
            }
            Event::Restore(epoch, Ok(manager)) if epoch == self.workspace.project_generation() => {
                self.workspace.apply_project_manager(manager);
                self.invalidate_io();
            }
            Event::Restore(_, Err(error)) => {
                tracing::error!(?error, "project state restoration failed");
            }
            Event::Settings(epoch, settings) if epoch == self.configuration => {
                if self.workspace.settings() == &settings {
                    return;
                }
                self.workspace.apply_settings(settings);
                let work = self.workspace.prepare_refresh();
                self.reload(work);
                self.invalidate_io();
            }
            _ => {}
        }
    }
    fn finish(&mut self, id: RequestId, result: Result<Effect>) {
        if self.queue.incoming.is_completed(&id) {
            self.running.remove(&id);
            return;
        }
        let Some(running) = self.running.remove(&id) else {
            return;
        };
        let stale = self.document_changed(&running.document)
            || if running.project_job {
                running.projects != self.workspace.project_generation()
            } else {
                matches!(&result,Ok(Effect::Source(contents)) if contents.loaded)
                    && running.analysis != self.workspace.generation()
            };
        if stale {
            self.respond(id, Err(HandlerError::ContentModified));
            return;
        }
        let result = result.and_then(|effect| match effect {
            Effect::Value(v) => Ok(v),
            Effect::Source(contents) => {
                if contents.loaded {
                    self.workspace
                        .apply_loaded_source(Arc::clone(&contents.source))?;
                    self.post_write();
                }
                Ok(Value::String(contents.source.contents().to_owned()))
            }
            Effect::Projects(manager) => {
                self.workspace.request_routing_refresh();
                if let Some(manager) = manager {
                    let _ = self.persist.send(manager.clone());
                    self.workspace.apply_project_manager(manager);
                }
                let work = self.workspace.prepare_refresh();
                self.reload(work);
                self.invalidate_io();
                Ok(Value::Null)
            }
        });
        self.respond(id, result);
    }
}
