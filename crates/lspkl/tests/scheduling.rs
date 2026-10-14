//! Main-loop scheduling guarantees over a memory connection: edits supersede
//! in-flight analysis, worker panics complete their work, and shutdown answers
//! every outstanding request exactly once.
#[path = "support/memory.rs"]
mod memory;

use fluent_uri::Uri;
use lsp_server::{Message, RequestId};
use lspkl::Workspace;
use lspkl_db::RootDatabase;
use lspkl_vfs::{FileError, Source, SourceLoader, StandardSourceLoader};
use memory::{DEADLINE, Peer};
use parking_lot::{Condvar, Mutex};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc,
    },
    time::Instant,
};

/// Blocks the calling thread until `set` raises the flag.
fn block_until(release: &(Mutex<bool>, Condvar)) {
    let (lock, condition) = release;
    let mut released = lock.lock();
    while !*released {
        condition.wait(&mut released);
    }
}
fn set(release: &(Mutex<bool>, Condvar)) {
    let (lock, condition) = release;
    *lock.lock() = true;
    condition.notify_all();
}

/// Delegates to the standard loader, except that `panic*.pkl` panics and
/// `blocked.pkl` waits until released.
struct FaultyLoader {
    standard: StandardSourceLoader,
    entered: Mutex<Option<mpsc::Sender<()>>>,
    released: Arc<(Mutex<bool>, Condvar)>,
}
impl FaultyLoader {
    fn new() -> (Arc<Self>, mpsc::Receiver<()>) {
        let (entered, entered_rx) = mpsc::channel();
        let loader = Self {
            standard: StandardSourceLoader::default(),
            entered: Mutex::new(Some(entered)),
            released: Arc::new((Mutex::new(false), Condvar::new())),
        };
        (Arc::new(loader), entered_rx)
    }
    fn release(&self) {
        set(&self.released);
    }
}
impl SourceLoader for FaultyLoader {
    fn load(&self, uri: &Uri<String>, cache_dir: &Path) -> Result<Arc<Source>, FileError> {
        let name = uri.path().as_str().rsplit('/').next().unwrap_or_default();
        if name.starts_with("panic") {
            panic!("injected loader panic for {uri}");
        }
        if name == "blocked.pkl" {
            if let Some(entered) = self.entered.lock().take() {
                entered.send(()).unwrap();
            }
            block_until(&self.released);
        }
        self.standard.load(uri, cache_dir)
    }
    fn list_files(&self, root: &Uri<String>) -> Result<Vec<Uri<String>>, FileError> {
        self.standard.list_files(root)
    }
}

fn file_uri(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().to_string()
}
fn hover(uri: &str, line: u32, character: u32) -> Value {
    json!({"textDocument":{"uri":uri},"position":{"line":line,"character":character}})
}
fn wait_for_diagnostics(peer: &Peer, version: i32) {
    loop {
        if let Message::Notification(n) = peer.message()
            && n.method == "textDocument/publishDiagnostics"
            && n.params["version"] == version
        {
            return;
        }
    }
}
fn workspace_with(loader: Arc<FaultyLoader>) -> Workspace {
    Workspace::with_database(RootDatabase::new(loader, Default::default()))
}

#[test]
fn edit_during_analysis_answers_content_modified_and_next_request_is_fresh() {
    // Idle -> (test) WaitingForWrite -> (next write) Armed -> (next query) Blocked,
    // released by the write after that. Writes happen only on the main loop and
    // queries only on the compute worker, so the edit is the sole way out.
    const IDLE: u8 = 0;
    const WAITING_FOR_WRITE: u8 = 1;
    const ARMED: u8 = 2;
    const BLOCKED: u8 = 3;
    let state = Arc::new(AtomicU8::new(IDLE));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let (entered_tx, entered) = mpsc::channel();
    let callback_state = state.clone();
    let entered_tx = Mutex::new(entered_tx);
    let db = RootDatabase::with_event_callback(
        Arc::new(StandardSourceLoader::default()),
        Default::default(),
        Some(Box::new(move |event| match event.kind {
            salsa::EventKind::DidSetCancellationFlag => {
                if callback_state.load(Ordering::SeqCst) == BLOCKED {
                    set(&release);
                } else {
                    let _ = callback_state.compare_exchange(
                        WAITING_FOR_WRITE,
                        ARMED,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    );
                }
            }
            salsa::EventKind::WillExecute { .. }
                if callback_state
                    .compare_exchange(ARMED, BLOCKED, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok() =>
            {
                entered_tx.lock().send(()).unwrap();
                block_until(&release);
            }
            _ => {}
        })),
    );
    let mut peer = Peer::start(Workspace::with_database(db), json!({"capabilities":{}}));
    let uri = "inmemory:///cancel.pkl";
    peer.open(uri, 1, "value: Int = 1\nanswer = value\n");
    wait_for_diagnostics(&peer, 1);
    state.store(WAITING_FOR_WRITE, Ordering::SeqCst);
    let change = |version: i32, text: &str| json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":text}]});
    // A fresh revision forces analysis to execute, and the armed query blocks
    // the compute worker while the hover is outstanding.
    peer.notify(
        "textDocument/didChange",
        change(2, "value: Int = 2\nanswer = value\n"),
    );
    let stale = peer.start_request("textDocument/hover", hover(uri, 1, 10));
    entered
        .recv_timeout(DEADLINE)
        .expect("compute worker must start analysis");
    peer.notify(
        "textDocument/didChange",
        change(3, "value: String = \"new\"\nanswer = value\n"),
    );
    let response = peer.response(stale);
    assert_eq!(response["error"]["code"], -32801, "{response}");
    let fresh = peer.request("textDocument/hover", hover(uri, 1, 10));
    let contents = fresh["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(contents.contains("value: String"), "{fresh}");
    peer.stop();
}

#[test]
fn panicking_input_load_completes_waiting_request() {
    let temp = tempfile::tempdir().unwrap();
    let (loader, _) = FaultyLoader::new();
    let mut peer = Peer::start(workspace_with(loader), json!({"capabilities":{}}));
    let main = file_uri(&temp.path().join("main.pkl"));
    peer.open(&main, 1, "import \"panic.pkl\"\nvalue = 1\n");
    let started = Instant::now();
    let response = peer.request("textDocument/hover", hover(&main, 1, 0));
    assert!(response.get("error").is_none(), "{response}");
    // The panic completes the load as failed instead of leaving the request to
    // the wait deadline.
    assert!(started.elapsed() < DEADLINE / 2, "{:?}", started.elapsed());
    let again = peer.request("textDocument/hover", hover(&main, 1, 0));
    assert!(again.get("error").is_none(), "{again}");
    peer.stop();
}

#[test]
fn panicking_external_jobs_fail_their_request_and_keep_every_worker() {
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join("real.pkl");
    std::fs::write(&real, "x = 1\n").unwrap();
    let (loader, _) = FaultyLoader::new();
    let mut peer = Peer::start(workspace_with(loader), json!({"capabilities":{}}));
    // More panics than I/O workers: a lost worker would leave the final request unanswered.
    for index in 0..6 {
        let uri = file_uri(&temp.path().join(format!("panic{index}.pkl")));
        let response = peer.request("pkl/fileContents", json!({"uri":uri}));
        assert_eq!(response["error"]["code"], -32603, "{response}");
    }
    let response = peer.request("pkl/fileContents", json!({"uri":file_uri(&real)}));
    assert_eq!(response["result"], "x = 1\n", "{response}");
    peer.stop();
}

#[test]
fn shutdown_answers_a_request_waiting_for_inputs_exactly_once() {
    let temp = tempfile::tempdir().unwrap();
    let (loader, entered) = FaultyLoader::new();
    let mut peer = Peer::start(workspace_with(loader.clone()), json!({"capabilities":{}}));
    let main = file_uri(&temp.path().join("main.pkl"));
    peer.open(&main, 1, "import \"blocked.pkl\"\nvalue = 1\n");
    let waiting = peer.start_request("textDocument/hover", hover(&main, 1, 0));
    entered
        .recv_timeout(DEADLINE)
        .expect("blocked input must start loading");
    let messages = peer.stop_collecting();
    loader.release();
    let answers: Vec<_> = messages
        .iter()
        .filter_map(|message| match message {
            Message::Response(response) if response.id == RequestId::from(waiting) => {
                Some(response)
            }
            _ => None,
        })
        .collect();
    assert_eq!(answers.len(), 1, "{answers:?}");
    let error = answers[0]
        .response_result
        .as_ref()
        .expect_err("cancelled by shutdown");
    assert_eq!(error.code, -32800);
    assert!(messages.iter().any(|message| matches!(
        message,
        Message::Response(response)
            if response.id == RequestId::from(i32::MAX) && response.response_result.is_ok()
    )));
}
