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

/// A real stdio peer. Spawning deliberately does not initialize the server.
/// The background reader answers configuration requests even while the caller
/// is idle; all other messages remain available for response/notification waits.
pub struct LspClient {
    child: Child,
    stdin: Arc<AsyncMutex<ChildStdin>>,
    incoming: mpsc::UnboundedReceiver<Result<Value, String>>,
    queued: VecDeque<Value>,
    stderr: Arc<Mutex<Vec<u8>>>,
    reader: JoinHandle<()>,
    stderr_reader: JoinHandle<()>,
    next_id: u64,
}

async fn send(stdin: &AsyncMutex<ChildStdin>, message: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(message)?;
    let mut stdin = stdin.lock().await;
    stdin
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await?;
    stdin.write_all(&body).await?;
    stdin.flush().await
}

impl LspClient {
    pub async fn spawn(binary: &Path, workspace: &Path) -> Self {
        let mut child = Command::new(binary)
            .current_dir(workspace)
            // Never discover a locally installed Pkl or the user's package cache.
            .env("PATH", "")
            .env("HOME", workspace)
            .env("USERPROFILE", workspace)
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
                let excess = bytes.len().saturating_sub(STDERR_LIMIT);
                bytes.drain(..excess);
            }
        });
        let (tx, incoming) = mpsc::unbounded_channel();
        let output = Arc::clone(&stdin);
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
                        if message.get("method").is_some() && message.get("id").is_some() =>
                    {
                        let response = if message["method"] == "workspace/configuration" {
                            let values: Vec<_> = message["params"]["items"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|_| Value::Null)
                                .collect();
                            json!({"jsonrpc":"2.0", "id":message["id"], "result":values})
                        } else {
                            json!({"jsonrpc":"2.0", "id":message["id"], "error":{"code":-32601,"message":"unsupported client request"}})
                        };
                        if let Err(err) = send(&output, &response).await {
                            let _ = tx.send(Err(err.to_string()));
                            break;
                        }
                    }
                    Ok(message) => {
                        if tx.send(Ok(message)).is_err() {
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
            next_id: 1,
        }
    }

    fn failure(&self, context: &str) -> String {
        format!(
            "{context}\nserver stderr:\n{}",
            String::from_utf8_lossy(&self.stderr.lock())
        )
    }

    async fn write(&self, value: Value) {
        match timeout(WAIT, send(&self.stdin, &value)).await {
            Ok(Ok(())) => {}
            result => panic!(
                "{}",
                self.failure(&format!("writing LSP message failed: {result:?}"))
            ),
        }
    }

    async fn matching(&mut self, description: &str, predicate: impl Fn(&Value) -> bool) -> Value {
        if let Some(index) = self.queued.iter().position(&predicate) {
            return self.queued.remove(index).unwrap();
        }
        let deadline = Instant::now() + WAIT;
        loop {
            match timeout_at(deadline, self.incoming.recv()).await {
                Ok(Some(Ok(message))) if predicate(&message) => return message,
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
        let id = self.next_id;
        self.next_id += 1;
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
