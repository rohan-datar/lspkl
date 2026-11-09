#![cfg(unix)]
mod support;

use lsp_server::{Message, Response};
use lspkl::Workspace;
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};
#[path = "support/memory.rs"]
mod memory;
use memory::Peer;

fn uri(path: &Path) -> String {
    lspkl_vfs::path_to_uri(path).unwrap().into_string()
}
fn wait(mut predicate: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < end, "fixture deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::write(root.join("PklProject"), "amends \"pkl:Project\"\n").unwrap();
        fs::create_dir(root.join("dep")).unwrap();
        fs::write(root.join("dep/value.pkl"), "value = 42\n").unwrap();
        fs::write(root.join("PklProject.deps.json"), json!({"schemaVersion":1,"resolvedDependencies":{
            "package://example.com/dep@1":{"type":"local","uri":"projectpackage://example.com/dep@1.0.0","path":"dep"}
        }}).to_string()).unwrap();
        let metadata = json!({"projectFileUri":uri(&root.join("PklProject")), "packageUri":null,
            "declaredDependencies":{"dep":"package://example.com/dep@1.0.0"},"evaluatorSettings":null});
        fs::write(root.join("metadata"), metadata.to_string()).unwrap();
        let script = format!(
            r#"#!/bin/sh
export HOME='{root}' XDG_CACHE_HOME='{root}/cache' PATH=''
printf '%s\n' "$*" >> '{root}/argv'
case "$1" in
--version) echo 'Pkl 0.33.0';;
project) exit 0;;
eval)
  : > '{root}/evaluating'
  while test -f '{root}/blocked'; do /bin/sleep 0.01; done
  /bin/cat '{root}/metadata';;
esac
"#,
            root = root.display()
        );
        fs::write(root.join("pkl"), script).unwrap();
        fs::set_permissions(root.join("pkl"), fs::Permissions::from_mode(0o755)).unwrap();
        Self { _temp: temp, root }
    }
    fn params(&self, auto: bool) -> Value {
        json!({"capabilities":{},"workspaceFolders":[{"uri":uri(&self.root),"name":"test"}],
            "initializationOptions":{"pkl":{"cli":{"path":self.root.join("pkl")},
                "cacheDir":self.root.join("cache"),"projects":{"autoSync":auto}}}})
    }
    fn start(&self, auto: bool) -> Peer {
        Peer::start(Workspace::new(), self.params(auto))
    }
    fn resolves(&self) -> usize {
        fs::read_to_string(self.root.join("argv"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("project resolve"))
            .count()
    }
    fn saved(&self) -> PathBuf {
        self.root.join(".lspkl/projects.json")
    }
    fn state(&self) {
        fs::create_dir_all(self.root.join(".lspkl")).unwrap();
        fs::write(self.saved(), json!({"schemaVersion":1,"pklProjects":[{
            "projectFile":"PklProject","packageUri":null,"declaredDependencies":{},"resolvedDependencies":null,"evaluatorSettings":null
        }]}).to_string()).unwrap();
    }
    fn watch(&self, peer: &Peer) {
        peer.notify(
            "workspace/didChangeWatchedFiles",
            json!({"changes":[{"uri":uri(&self.root.join("PklProject")),"type":2}]}),
        );
    }
}
fn quiet() {
    std::thread::sleep(Duration::from_millis(600));
}

#[test]
fn startup_syncs_once_and_clears_dependency_diagnostics_without_edit() {
    let f = Fixture::new();
    fs::write(f.root.join("blocked"), "").unwrap();
    let peer = f.start(true);
    let main = uri(&f.root.join("main.pkl"));
    peer.open(
        &main,
        1,
        "import \"@dep/value.pkl\" as dep\nresult = dep.value\n",
    );
    wait(|| f.root.join("evaluating").exists());
    loop {
        if let Message::Notification(n) = peer.message()
            && n.method == "textDocument/publishDiagnostics"
        {
            assert!(
                !n.params["diagnostics"].as_array().unwrap().is_empty(),
                "{n:?}"
            );
            break;
        }
    }
    fs::remove_file(f.root.join("blocked")).unwrap();
    loop {
        if let Message::Notification(n) = peer.message()
            && n.method == "textDocument/publishDiagnostics"
            && n.params["diagnostics"] == json!([])
        {
            break;
        }
    }
    wait(|| f.saved().exists());
    quiet();
    assert_eq!(f.resolves(), 1);
    peer.stop();
}

#[test]
fn fresh_restored_metadata_does_not_sync() {
    let f = Fixture::new();
    f.state();
    let peer = f.start(true);
    quiet();
    assert_eq!(f.resolves(), 0);
    peer.stop();
}
#[test]
fn stale_project_and_dependency_metadata_sync() {
    for input in ["PklProject", "PklProject.deps.json"] {
        let f = Fixture::new();
        f.state();
        let old = SystemTime::now() - Duration::from_secs(60);
        for name in ["PklProject", "PklProject.deps.json"] {
            fs::File::open(f.root.join(name))
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        fs::File::open(f.root.join(input))
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(SystemTime::now() + Duration::from_secs(1)),
            )
            .unwrap();
        let peer = f.start(true);
        wait(|| f.resolves() == 1);
        quiet();
        assert_eq!(f.resolves(), 1);
        peer.stop();
    }
}
#[test]
fn watched_project_burst_coalesces() {
    let f = Fixture::new();
    f.state();
    let peer = f.start(true);
    quiet();
    for _ in 0..20 {
        f.watch(&peer);
    }
    wait(|| f.resolves() == 1);
    quiet();
    assert_eq!(f.resolves(), 1);
    peer.stop();
}
#[test]
fn disabled_auto_sync_never_invokes_cli() {
    let f = Fixture::new();
    let peer = f.start(false);
    quiet();
    for _ in 0..5 {
        f.watch(&peer);
    }
    quiet();
    assert_eq!(f.resolves(), 0);
    peer.stop();
}
#[test]
fn missing_cli_warns_once_without_retry_loop() {
    let f = Fixture::new();
    fs::remove_file(f.root.join("pkl")).unwrap();
    let peer = f.start(true);
    let Message::Notification(n) = peer.message() else {
        panic!("expected warning")
    };
    assert_eq!(n.method, "window/showMessage");
    assert_eq!(n.params["type"], 2);
    assert!(
        n.params["message"]
            .as_str()
            .unwrap()
            .contains("pkl.cli.path")
    );
    f.watch(&peer);
    quiet();
    assert!(peer.connection.receiver.try_recv().is_err());
    assert!(!f.saved().exists());
    peer.stop();
}
#[test]
fn superseded_auto_sync_is_not_persisted() {
    let f = Fixture::new();
    fs::write(f.root.join("blocked"), "").unwrap();
    let peer = f.start(true);
    wait(|| f.root.join("evaluating").exists());
    peer.notify("workspace/didChangeConfiguration", json!({"settings":{"pkl":{"cli":{"path":f.root.join("pkl")},"cacheDir":f.root.join("cache"),"projects":{"autoSync":false}}}}));
    // A request round trip ensures the configuration notification was processed.
    let mut peer = peer;
    peer.request("textDocument/hover", json!({"textDocument":{"uri":uri(&f.root.join("main.pkl"))},"position":{"line":0,"character":0}}));
    fs::remove_file(f.root.join("blocked")).unwrap();
    quiet();
    assert_eq!(f.resolves(), 1);
    assert!(!f.saved().exists());
    peer.stop();
}
#[test]
fn folder_addition_syncs_new_project() {
    let f = Fixture::new();
    let mut params = f.params(true);
    params["workspaceFolders"] = json!([]);
    let peer = Peer::start(Workspace::new(), params);
    quiet();
    assert_eq!(f.resolves(), 0);
    peer.notify(
        "workspace/didChangeWorkspaceFolders",
        json!({"event":{"removed":[],"added":[{"uri":uri(&f.root),"name":"test"}]}}),
    );
    wait(|| f.saved().exists());
    assert_eq!(f.resolves(), 1);
    peer.stop();
}
#[test]
fn startup_waits_for_pulled_cli_settings() {
    let f = Fixture::new();
    let mut params = f.params(true);
    params["capabilities"] = json!({"workspace":{"configuration":true}});
    params["initializationOptions"]["pkl"]["cli"]["path"] = json!(f.root.join("missing"));
    let peer = Peer::start(Workspace::new(), params);
    let Message::Request(request) = peer
        .connection
        .receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
    else {
        panic!("configuration request")
    };
    assert_eq!(request.method, "workspace/configuration");
    quiet();
    assert_eq!(f.resolves(), 0);
    peer.connection
        .sender
        .send(
            Response::new_ok(
                request.id,
                json!([f.root.join("pkl"), null, null, null, null, true]),
            )
            .into(),
        )
        .unwrap();
    wait(|| f.saved().exists());
    assert_eq!(f.resolves(), 1);
    peer.stop();
}

#[test]
fn manual_sync_waits_for_automatic_sync() {
    let f = Fixture::new();
    fs::write(f.root.join("blocked"), "").unwrap();
    let mut peer = f.start(true);
    wait(|| f.root.join("evaluating").exists());
    let id = peer.start_request("pkl/syncProjects", Value::Null);
    quiet();
    assert_eq!(f.resolves(), 1);
    fs::remove_file(f.root.join("blocked")).unwrap();
    let response = peer.response(id);
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(f.resolves(), 2);
    peer.stop();
}

#[test]
fn cli_error_warns_once_for_repeated_failure() {
    let f = Fixture::new();
    fs::write(
        f.root.join("pkl"),
        "#!/bin/sh\necho 'broken project' >&2\nexit 1\n",
    )
    .unwrap();
    let peer = f.start(true);
    let Message::Notification(n) = peer.message() else {
        panic!("expected warning")
    };
    assert_eq!(n.method, "window/showMessage");
    assert!(
        n.params["message"]
            .as_str()
            .unwrap()
            .contains("broken project")
    );
    f.watch(&peer);
    quiet();
    assert!(peer.connection.receiver.try_recv().is_err());
    peer.stop();
}

#[test]
fn rejected_configuration_pull_uses_initialization_options() {
    let f = Fixture::new();
    let mut params = f.params(true);
    params["capabilities"] = json!({"workspace":{"configuration":true}});
    let peer = Peer::start(Workspace::new(), params);
    let Message::Request(request) = peer
        .connection
        .receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
    else {
        panic!("configuration request")
    };
    peer.connection
        .sender
        .send(Response::new_err(request.id, -32601, "configuration unavailable".into()).into())
        .unwrap();
    wait(|| f.saved().exists());
    assert_eq!(f.resolves(), 1);
    peer.stop();
}

#[test]
fn auto_sync_setting_is_nested_default_true_and_null_pull_keeps_push() {
    let mut workspace = Workspace::new();
    assert!(workspace.settings().projects.auto_sync);
    lspkl::handlers::lifecycle::did_change_configuration(
        &mut workspace,
        serde_json::from_value(json!({"settings":{"pkl":{"projects":{"autoSync":false}}}}))
            .unwrap(),
    )
    .unwrap();
    assert!(!workspace.settings().projects.auto_sync);
    workspace
        .apply_pulled_settings(&[
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        ])
        .unwrap();
    assert!(!workspace.settings().projects.auto_sync);
    workspace
        .apply_pulled_settings(&[
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            json!(true),
        ])
        .unwrap();
    assert!(workspace.settings().projects.auto_sync);
}

/// Editors report the PklProject.deps.json that `pkl project resolve` rewrites
/// during every sync. That event must not supersede the sync that caused it,
/// or the result is dropped, nothing is persisted, and syncing never stops.
#[test]
fn deps_file_rewritten_by_resolve_does_not_restart_sync() {
    let f = Fixture::new();
    fs::write(f.root.join("blocked"), "").unwrap();
    let peer = f.start(true);
    wait(|| f.root.join("evaluating").exists());
    // Like an editor watcher reporting resolve's rewrite while eval runs.
    peer.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes":[{"uri":uri(&f.root.join("PklProject.deps.json")),"type":2}]}),
    );
    fs::remove_file(f.root.join("blocked")).unwrap();
    wait(|| f.saved().exists());
    // And again after the result was applied.
    peer.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes":[{"uri":uri(&f.root.join("PklProject.deps.json")),"type":2}]}),
    );
    quiet();
    quiet();
    assert_eq!(f.resolves(), 1, "the sync must not restart itself");
    peer.stop();
}

#[test]
fn deps_file_event_does_not_reject_a_manual_sync() {
    let f = Fixture::new();
    fs::write(f.root.join("blocked"), "").unwrap();
    let mut peer = f.start(false);
    let sync = peer.start_request("pkl/syncProjects", Value::Null);
    wait(|| f.root.join("evaluating").exists());
    peer.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes":[{"uri":uri(&f.root.join("PklProject.deps.json")),"type":2}]}),
    );
    fs::remove_file(f.root.join("blocked")).unwrap();
    let response = peer.response(sync);
    assert!(response.get("error").is_none(), "{response}");
    wait(|| f.saved().exists());
    peer.stop();
}
