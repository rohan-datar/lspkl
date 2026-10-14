// Shared by tests and examples, each using a different subset of helpers.
#![allow(dead_code)]

use parking_lot::Mutex;
use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout, timeout_at};

const WAIT: Duration = Duration::from_secs(15);
const STDERR_LIMIT: usize = 64 * 1024;

/// Resolve fixture symlinks without retaining Windows verbatim path prefixes.
pub fn canonical_root(path: &Path) -> std::path::PathBuf {
    let canonical = path.canonicalize().expect("fixture root should exist");
    #[cfg(windows)]
    {
        use std::path::{Component, PathBuf, Prefix};
        let mut components = canonical.components();
        let prefix = match components.next() {
            Some(Component::Prefix(prefix)) => prefix.kind(),
            _ => return canonical,
        };
        let mut root = match prefix {
            Prefix::VerbatimDisk(drive) => PathBuf::from(format!("{}:\\", char::from(drive))),
            Prefix::VerbatimUNC(server, share) => {
                let mut root = PathBuf::from(r"\\");
                root.push(server);
                root.push(share);
                root
            }
            _ => return canonical,
        };
        for component in components {
            if !matches!(component, Component::RootDir) {
                root.push(component.as_os_str());
            }
        }
        return root;
    }
    #[cfg(not(windows))]
    canonical
}

/// Apply the synchronous parts of initialization for direct handler fixtures.
pub fn initialize_workspace(
    workspace: &mut lspkl::Workspace,
    params: lsp_types::InitializeParams,
) -> lspkl::handlers::Result<lsp_types::InitializeResult> {
    let result = lspkl::handlers::lifecycle::initialize(workspace, &params);
    let mut settings = workspace.settings().clone();
    settings.resolve_paths(workspace.project_manager().workspace_folders());
    if params.initialization_options.is_some() {
        workspace.apply_settings(settings);
    }
    let manager = lspkl::handlers::lifecycle::restore_workspaces(workspace.project_manager())?;
    workspace.apply_project_manager(manager);
    Ok(result)
}
/// Prepare synchronous semantic fixtures without fetching synthetic network URLs.
/// Use Workspace writes (rather than the database fixture helper) so every loaded
/// input also advances the analysis generation.
pub fn preload(workspace: &mut lspkl::Workspace) {
    let loader = workspace.database().source_loader_handle();
    let https = lspkl_vfs::StandardSourceLoader::new();
    let roots = workspace.database().directory_roots();
    for root in roots {
        workspace
            .apply_io(lspkl_db::IoUpdate::Directory(
                lspkl_db::DirectoryUpdate::load(&*loader, &root),
            ))
            .expect("fixture directory input should apply");
    }
    loop {
        for file in workspace.database().all_files() {
            let _ = lspkl_db::import_graph(workspace.database(), file);
        }
        let jobs = workspace.database().pending_io();
        if jobs.is_empty() {
            break;
        }
        for job in jobs {
            let update = match job {
                lspkl_db::PendingIo::Source { uri, cache_dir }
                    if uri.scheme().as_str() == "https" =>
                {
                    lspkl_db::IoUpdate::Source {
                        text: loader
                            .load(&uri, &cache_dir)
                            .ok()
                            .map(|source| source.text.clone()),
                        uri,
                    }
                }
                job => job.load(&*loader, &https),
            };
            workspace
                .apply_io(update)
                .expect("fixture input should apply");
        }
    }
}

/// A real stdio peer. Spawning deliberately does not initialize the server.
/// The background reader answers configuration requests even while the caller
/// is idle; all other messages remain available for response/notification waits.
pub struct LspClient {
    child: Child,
    stdin: Arc<AsyncMutex<ChildStdin>>,
    incoming: mpsc::UnboundedReceiver<Result<ObservedMessage, String>>,
    queued: VecDeque<ObservedMessage>,
    stderr: Arc<Mutex<Vec<u8>>>,
    reader: JoinHandle<()>,
    stderr_reader: JoinHandle<()>,
    next_id: Arc<std::sync::atomic::AtomicU64>,
}

/// Timestamped by the reader before consumer-side queuing.
#[derive(Debug)]
pub struct ObservedMessage {
    pub message: Value,
    pub received_at: Instant,
}

#[derive(Clone)]
pub struct LspSender {
    stdin: Arc<AsyncMutex<ChildStdin>>,
    next_id: Arc<std::sync::atomic::AtomicU64>,
}

#[derive(Clone, Copy)]
pub struct PendingRequest {
    pub id: u64,
    pub sent_at: Instant,
}

impl LspSender {
    /// Sends without waiting for a response; sent_at is after frame flush.
    pub async fn start_request(&self, method: &str, params: Value) -> PendingRequest {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let sent_at = self
            .write(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))
            .await;
        PendingRequest { id, sent_at }
    }

    pub async fn notify(&self, method: &str, params: Value) -> Instant {
        self.write(json!({"jsonrpc":"2.0", "method":method, "params":params}))
            .await
    }

    async fn write(&self, message: Value) -> Instant {
        timeout(WAIT, send(&self.stdin, &message))
            .await
            .expect("timed out writing LSP message")
            .expect("failed writing LSP message")
    }
}

async fn send(stdin: &AsyncMutex<ChildStdin>, message: &Value) -> std::io::Result<Instant> {
    let body = serde_json::to_vec(message)?;
    let mut stdin = stdin.lock().await;
    stdin
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await?;
    stdin.write_all(&body).await?;
    stdin.flush().await?;
    Ok(Instant::now())
}

impl LspClient {
    pub async fn spawn(binary: &Path, workspace: &Path) -> Self {
        Self::spawn_with_stderr_limit(binary, workspace, STDERR_LIMIT).await
    }

    pub async fn spawn_with_stderr_limit(
        binary: &Path,
        workspace: &Path,
        stderr_limit: usize,
    ) -> Self {
        Self::spawn_mode(binary, workspace, stderr_limit, true, &[]).await
    }

    pub async fn spawn_manual_configuration(binary: &Path, workspace: &Path) -> Self {
        Self::spawn_mode(binary, workspace, STDERR_LIMIT, false, &[]).await
    }

    pub async fn spawn_with_env(binary: &Path, workspace: &Path, env: &[(&str, &Path)]) -> Self {
        Self::spawn_mode(binary, workspace, STDERR_LIMIT, true, env).await
    }

    async fn spawn_mode(
        binary: &Path,
        workspace: &Path,
        stderr_limit: usize,
        auto_configuration: bool,
        env: &[(&str, &Path)],
    ) -> Self {
        let mut child = Command::new(binary)
            .current_dir(workspace)
            // Never discover a locally installed Pkl or the user's package cache.
            .env("PATH", "")
            .env("HOME", workspace)
            .env("USERPROFILE", workspace)
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap_or_else(|err| panic!("could not spawn {}: {err}", binary.display()));
        let stdin = Arc::new(AsyncMutex::new(child.stdin.take().unwrap()));
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut stderr_pipe = child.stderr.take().unwrap();
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&stderr);
        let stderr_reader = tokio::spawn(async move {
            let mut chunk = [0; 4096];
            while let Ok(count) = stderr_pipe.read(&mut chunk).await {
                if count == 0 {
                    break;
                }
                let mut bytes = captured.lock();
                bytes.extend_from_slice(&chunk[..count]);
                let excess = bytes.len().saturating_sub(stderr_limit);
                bytes.drain(..excess);
            }
        });
        let (tx, incoming) = mpsc::unbounded_channel();
        let output = Arc::clone(&stdin);
        // Like editors without `pkl.*` user settings, answer every requested
        // section with null; the server must keep its explicit settings.
        let reader = tokio::spawn(async move {
            loop {
                let result = async {
                    let mut length = None;
                    loop {
                        let mut line = String::new();
                        if stdout
                            .read_line(&mut line)
                            .await
                            .map_err(|e| e.to_string())?
                            == 0
                        {
                            return Err("server closed stdout".to_owned());
                        }
                        if line == "\r\n" {
                            break;
                        }
                        let (name, value) = line.split_once(':').ok_or("invalid LSP header")?;
                        if name.eq_ignore_ascii_case("Content-Length") {
                            length =
                                Some(value.trim().parse::<usize>().map_err(|e| e.to_string())?);
                        }
                    }
                    let length = length.ok_or("missing Content-Length")?;
                    if length > 16 * 1024 * 1024 {
                        return Err("oversized LSP frame".to_owned());
                    }
                    let mut body = vec![0; length];
                    stdout
                        .read_exact(&mut body)
                        .await
                        .map_err(|e| e.to_string())?;
                    serde_json::from_slice::<Value>(&body).map_err(|e| e.to_string())
                }
                .await;
                match result {
                    Ok(message)
                        if !auto_configuration
                            && message["method"] == "workspace/configuration" =>
                    {
                        let _ = tx.send(Ok(ObservedMessage {
                            message,
                            received_at: Instant::now(),
                        }));
                    }
                    Ok(message)
                        if message.get("method").is_some() && message.get("id").is_some() =>
                    {
                        let response = if message["method"] == "workspace/configuration" {
                            let items = message["params"]["items"].as_array().unwrap().len();
                            json!({"jsonrpc":"2.0", "id":message["id"], "result":vec![Value::Null; items]})
                        } else if message["method"] == "client/registerCapability" {
                            json!({"jsonrpc":"2.0", "id":message["id"], "result":null})
                        } else {
                            json!({"jsonrpc":"2.0", "id":message["id"], "error":{"code":-32601,"message":"unsupported client request"}})
                        };
                        if let Err(err) = send(&output, &response).await {
                            let _ = tx.send(Err(err.to_string()));
                            break;
                        }
                        if message["method"] == "client/registerCapability" {
                            let _ = tx.send(Ok(ObservedMessage {
                                message,
                                received_at: Instant::now(),
                            }));
                        }
                    }
                    Ok(message) => {
                        let observed = ObservedMessage {
                            message,
                            received_at: Instant::now(),
                        };
                        if tx.send(Ok(observed)).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err));
                        break;
                    }
                }
            }
        });
        Self {
            child,
            stdin,
            incoming,
            queued: VecDeque::new(),
            stderr,
            reader,
            stderr_reader,
            next_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        }
    }
    pub async fn respond(&self, id: Value, result: Value) {
        self.write(json!({"jsonrpc":"2.0","id":id,"result":result}))
            .await;
    }

    pub async fn response(&mut self, id: u64) -> Value {
        self.matching("request response", |v| {
            v["id"] == id && v.get("method").is_none()
        })
        .await
    }

    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock()).into_owned()
    }

    fn failure(&self, context: &str) -> String {
        format!(
            "{context}\nserver stderr:\n{}",
            String::from_utf8_lossy(&self.stderr.lock())
        )
    }

    async fn write(&self, value: Value) {
        match timeout(WAIT, send(&self.stdin, &value)).await {
            Ok(Ok(_)) => {}
            result => panic!(
                "{}",
                self.failure(&format!("writing LSP message failed: {result:?}"))
            ),
        }
    }

    async fn matching(&mut self, description: &str, predicate: impl Fn(&Value) -> bool) -> Value {
        if let Some(index) = self.queued.iter().position(|item| predicate(&item.message)) {
            return self.queued.remove(index).unwrap().message;
        }
        let deadline = Instant::now() + WAIT;
        loop {
            match timeout_at(deadline, self.incoming.recv()).await {
                Ok(Some(Ok(message))) if predicate(&message.message) => return message.message,
                Ok(Some(Ok(message))) => self.queued.push_back(message),
                result => panic!(
                    "{}",
                    self.failure(&format!(
                        "waiting for {description}: {result:?}; queued: {:?}",
                        self.queued
                    ))
                ),
            }
        }
    }

    /// Returns the entire response, including JSON-RPC errors.
    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.write(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))
            .await;
        self.matching(method, |value| value["id"] == id).await
    }

    pub async fn notify(&mut self, method: &str, params: Value) {
        self.write(json!({"jsonrpc":"2.0", "method":method, "params":params}))
            .await;
    }

    pub async fn notification(&mut self, method: &str) -> Value {
        self.matching(method, |value| value["method"] == method)
            .await
    }

    pub fn sender(&self) -> LspSender {
        LspSender {
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
        }
    }

    /// Drains queued messages first, retaining original receipt timestamps.
    pub async fn observe_until(&mut self, deadline: Instant) -> Option<ObservedMessage> {
        if let Some(message) = self.queued.pop_front() {
            return Some(message);
        }
        match timeout_at(deadline, self.incoming.recv()).await {
            Ok(Some(Ok(message))) => Some(message),
            Err(_) => None,
            result => panic!(
                "{}",
                self.failure(&format!("observing LSP message: {result:?}"))
            ),
        }
    }

    pub async fn shutdown(mut self) {
        let response = self.request("shutdown", Value::Null).await;
        assert!(response.get("error").is_none(), "{response}");
        self.notify("exit", Value::Null).await;
        match timeout(WAIT, self.child.wait()).await {
            Ok(Ok(status)) if status.success() => {}
            result => panic!(
                "{}",
                self.failure(&format!("server did not exit cleanly: {result:?}"))
            ),
        }
        match timeout(WAIT, &mut self.stderr_reader).await {
            Ok(Ok(())) => {}
            result => panic!(
                "{}",
                self.failure(&format!("draining server stderr failed: {result:?}"))
            ),
        }
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        self.reader.abort();
        self.stderr_reader.abort();
    }
}
