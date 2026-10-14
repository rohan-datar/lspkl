//! Consumer-visible command, diagnostic, and cancellation behavior over memory connections.
#![cfg(unix)]
#[path = "support/memory.rs"]
mod memory;

use lsp_server::Message;
use lspkl::Workspace;
use memory::Peer;
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn wait_for(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(Instant::now() < deadline, "missing {}", path.display());
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn sync_projects_round_trip_via_jsonrpc() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let project_file = workspace.join("PklProject");
    fs::write(&project_file, "amends \"pkl:Project\"\n").unwrap();
    fs::write(
        workspace.join("PklProject.deps.json"),
        r#"{"schemaVersion":1,"resolvedDependencies":{}}"#,
    )
    .unwrap();
    let metadata = json!({"projectFileUri":url::Url::from_file_path(&project_file).unwrap().as_str(),"packageUri":null,"declaredDependencies":{},"evaluatorSettings":null});
    let calls = temp.path().join("calls.txt");
    let binary = temp.path().join("pkl");
    fs::write(&binary,format!("#!/bin/sh\nprintf '%s\\n' \"$1\" >> {}\ncase \"$1\" in\nproject) exit 0;;\neval) printf '%s' {};;\n*) exit 9;;\nesac\n",shell_quote(&calls.to_string_lossy()),shell_quote(&metadata.to_string()))).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let mut peer = Peer::start(
        Workspace::new(),
        json!({"capabilities":{},"workspaceFolders":[{"uri":url::Url::from_directory_path(&workspace).unwrap().as_str(),"name":"fixture"}],"initializationOptions":{"cli":{"path":binary}}}),
    );
    let response = peer.request("pkl/syncProjects", Value::Null);
    assert!(response.get("error").is_none(), "{response}");
    let state_file = workspace.join(".lspkl/projects.json");
    wait_for(&state_file);
    let mut manager = pkl_project::project_manager::ProjectManager::default();
    manager.initialize(vec![workspace]);
    manager.load_state(&state_file).unwrap();
    assert_eq!(manager.projects().count(), 1);
    assert_eq!(
        fs::read_to_string(calls)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["project", "eval"]
    );
    peer.stop();
}
#[test]
fn download_package_round_trip_via_jsonrpc() {
    let temp = tempfile::tempdir().unwrap();
    let calls = temp.path().join("download-args.txt");
    let binary = temp.path().join("pkl");
    fs::write(&binary,format!("#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\nif [ \"$5\" = 'package://example.com/fail@1.0.0' ]; then echo 'test download failure' >&2; exit 7; fi\n",shell_quote(&calls.to_string_lossy()))).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let cache = temp.path().join("cache");
    let mut peer = Peer::start(
        Workspace::new(),
        json!({"capabilities":{},"initializationOptions":{"cli":{"path":binary},"cacheDir":cache}}),
    );
    for (uri, code) in [
        ("package://example.com/ok@1.0.0", None),
        ("not-a-package-uri", Some(-32602)),
        ("package://example.com/fail@1.0.0", Some(-32603)),
    ] {
        let response = peer.request("pkl/downloadPackage", Value::String(uri.into()));
        if let Some(code) = code {
            assert_eq!(response["error"]["code"], code, "{response}");
            if code == -32603 {
                assert!(
                    response["error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains("test download failure")
                );
            }
        } else {
            assert!(response.get("error").is_none(), "{response}");
            assert_eq!(response["result"], Value::Null);
        }
    }
    assert_eq!(
        fs::read_to_string(calls)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [
            "download-package",
            "--no-transitive",
            "--cache-dir",
            cache.to_str().unwrap(),
            "package://example.com/ok@1.0.0",
            "download-package",
            "--no-transitive",
            "--cache-dir",
            cache.to_str().unwrap(),
            "package://example.com/fail@1.0.0"
        ]
    );
    peer.stop();
}
#[test]
fn stale_diagnostic_generation_waits_for_close_and_settings_refresh() {
    let mut peer = Peer::start(Workspace::new(), json!({"capabilities":{}}));
    let uri = "inmemory:///main.pkl";
    peer.open(uri, 1, "value: Int = \"wrong\"\n");
    peer.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}));
    peer.open(uri, 2, "value: Int = \"wrong\"\n");
    peer.notify(
        "workspace/didChangeConfiguration",
        json!({"settings":{"modulepath":[]}}),
    );
    peer.notify("textDocument/didChange",json!({"textDocument":{"uri":uri,"version":3},"contentChanges":[{"text":"value: String = \"latest\"\n"}]}));
    let id = peer.start_request("pkl/fileContents", json!({"uri":uri}));
    let mut responded = false;
    let mut published = false;
    while !responded || !published {
        match peer.message() {
            Message::Response(r) if r.id == id.into() => {
                let r = serde_json::to_value(r).unwrap();
                assert_eq!(r["result"], "value: String = \"latest\"\n");
                responded = true;
            }
            Message::Notification(n)
                if n.method == "textDocument/publishDiagnostics" && n.params["version"] == 3 =>
            {
                assert_eq!(n.params["diagnostics"], json!([]));
                published = true;
            }
            _ => {}
        }
    }
    peer.stop();
}
#[test]
fn sync_completion_survives_document_edits_and_installs_project_imports() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let project = root.join("PklProject");
    fs::write(&project, "amends \"pkl:Project\"\n").unwrap();
    fs::write(
        root.join("PklProject.deps.json"),
        r#"{"schemaVersion":1,"resolvedDependencies":{}}"#,
    )
    .unwrap();
    fs::create_dir(root.join("modules")).unwrap();
    fs::write(root.join("modules/library.pkl"), "value: Int = 1\n").unwrap();
    let started = root.join("started");
    let release = root.join("release");
    let binary = root.join("pkl");
    let metadata = json!({"projectFileUri":url::Url::from_file_path(project).unwrap().as_str(),"packageUri":null,"declaredDependencies":{},"evaluatorSettings":{"modulePath":["modules"]}});
    fs::write(&binary,format!("#!/bin/sh\ncase \"$1\" in\nproject) exit 0;;\neval) : > {}; while [ ! -e {} ]; do /bin/sleep 0.01; done; printf '%s' {};;\n*) exit 9;;\nesac\n",shell_quote(&started.to_string_lossy()),shell_quote(&release.to_string_lossy()),shell_quote(&metadata.to_string()))).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let mut peer = Peer::start(
        Workspace::new(),
        json!({"capabilities":{},"workspaceFolders":[{"uri":url::Url::from_directory_path(root).unwrap().as_str(),"name":"fixture"}],"initializationOptions":{"cli":{"path":binary}}}),
    );
    let main = url::Url::from_file_path(root.join("main.pkl"))
        .unwrap()
        .to_string();
    let source = "import \"modulepath:/library.pkl\" as library\nanswer = library.value\n";
    peer.open(&main, 1, source);
    let sync = peer.start_request("pkl/syncProjects", Value::Null);
    wait_for(&started);
    peer.notify("textDocument/didChange",json!({"textDocument":{"uri":main,"version":2},"contentChanges":[{"text":format!("// changed during CLI\n{source}")}]}));
    fs::write(release, "").unwrap();
    let response = peer.response(sync);
    assert!(response.get("error").is_none(), "{response}");
    let hover = peer.request(
        "textDocument/hover",
        json!({"textDocument":{"uri":main},"position":{"line":2,"character":18}}),
    );
    assert!(
        hover["result"]["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("value: Int"),
        "{hover}"
    );
    peer.stop();
}

#[test]
fn watched_unregistered_project_invalidates_sync_without_persisting_rejected_state() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let project = root.join("PklProject");
    fs::write(&project, "amends \"pkl:Project\"\n").unwrap();
    let started = root.join("started");
    let release = root.join("release");
    let binary = root.join("pkl");
    let metadata = json!({"projectFileUri":url::Url::from_file_path(&project).unwrap().as_str(),"packageUri":null,"declaredDependencies":{},"evaluatorSettings":null});
    fs::write(&binary,format!("#!/bin/sh\ncase \"$1\" in\nproject) exit 0;;\neval) : > {}; while [ ! -e {} ]; do /bin/sleep 0.01; done; printf '%s' {};;\nesac\n",shell_quote(&started.to_string_lossy()),shell_quote(&release.to_string_lossy()),shell_quote(&metadata.to_string()))).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let mut peer = Peer::start(
        Workspace::new(),
        json!({"capabilities":{},"workspaceFolders":[{"uri":url::Url::from_directory_path(root).unwrap().as_str(),"name":"fixture"}],"initializationOptions":{"cli":{"path":binary}}}),
    );
    let sync = peer.start_request("pkl/syncProjects", Value::Null);
    wait_for(&started);
    peer.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes":[{"uri":url::Url::from_file_path(&project).unwrap().as_str(),"type":2}]}),
    );
    let barrier=peer.request("textDocument/hover",json!({"textDocument":{"uri":"inmemory:///unopened.pkl"},"position":{"line":0,"character":0}}));
    assert!(barrier.get("error").is_none(), "{barrier}");
    fs::write(&release, "").unwrap();
    let response = peer.response(sync);
    assert_eq!(response["error"]["code"], -32801, "{response}");
    assert!(
        !root.join(".lspkl/projects.json").exists(),
        "rejected sync must not persist"
    );
    let accepted = peer.request("pkl/syncProjects", Value::Null);
    assert!(accepted.get("error").is_none(), "{accepted}");
    wait_for(&root.join(".lspkl/projects.json"));
    peer.stop();
}
