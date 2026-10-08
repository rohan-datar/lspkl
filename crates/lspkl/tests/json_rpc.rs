//! End-to-end JSON-RPC smoke test.
//!
//! Wires the server's stdin/stdout to in-memory `tokio::io::duplex` streams
//! and speaks LSP protocol over them. Slower than the direct-call tests in
//! `lifecycle.rs`, but verifies the JSON-RPC framing actually works.

#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

#[cfg(unix)]
use pkl_project::cli::PklCli;

use lspkl::{PklBackend, ServerState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower_lsp_server::{LspService, Server};

#[tokio::test]
async fn initialize_round_trip_via_jsonrpc() {
    // Plumb client <-> server via two in-memory pipes.
    let (mut client_tx, server_rx) = tokio::io::duplex(4096);
    let (server_tx, mut client_rx) = tokio::io::duplex(4096);

    let state = Arc::new(ServerState::new());
    let (service, socket) =
        LspService::new(move |client| PklBackend::new(Arc::clone(&state), Some(client)));

    let server_task = tokio::spawn(async move {
        Server::new(server_rx, server_tx, socket)
            .serve(service)
            .await;
    });

    // 1. initialize
    write_message(
        &mut client_tx,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#,
    )
    .await;

    let response = read_message(&mut client_rx).await;
    let json: serde_json::Value = serde_json::from_str(&response).expect("response must be JSON");

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 1);
    assert!(
        json["result"]["capabilities"].is_object(),
        "initialize response must carry a capabilities object, got: {response}"
    );

    // 2. shutdown
    write_message(
        &mut client_tx,
        r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#,
    )
    .await;
    let shutdown_resp = read_message(&mut client_rx).await;
    let json: serde_json::Value = serde_json::from_str(&shutdown_resp).unwrap();
    assert_eq!(json["id"], 2);

    // 3. exit notification (no id, no response expected). Tower-lsp-server's
    //    Server stops serving once `exit` arrives.
    write_message(&mut client_tx, r#"{"jsonrpc":"2.0","method":"exit"}"#).await;

    // Drop our writer so the server sees EOF if it's still polling the read
    // half for some reason.
    drop(client_tx);

    // Bound the wait so a regression in shutdown doesn't hang CI forever.
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server did not finish within 2 seconds after exit")
        .expect("server task panicked");
}

#[cfg(unix)]
#[tokio::test]
async fn sync_projects_round_trip_via_jsonrpc() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let project_file = workspace.join("PklProject");
    fs::write(&project_file, "amends \"pkl:0.7.0\"\n").unwrap();
    fs::write(
        workspace.join("PklProject.deps.json"),
        r#"{"schemaVersion":1,"resolvedDependencies":{}}"#,
    )
    .unwrap();
    let metadata = serde_json::json!({
        "projectFileUri": url::Url::from_file_path(&project_file).unwrap().to_string(),
        "packageUri": null,
        "declaredDependencies": {},
        "evaluatorSettings": null
    });
    let calls = temp.path().join("calls.txt");
    let staged = temp.path().join("pkl-stub.tmp");
    let binary = temp.path().join("pkl-stub");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$1\" >> {}\ncase \"$1\" in\n  project) exit 0 ;;\n  eval) printf '%s' {} ;;\n  *) exit 9 ;;\nesac\n",
        shell_quote(&calls.to_string_lossy()),
        shell_quote(&metadata.to_string()),
    );
    fs::write(&staged, script).unwrap();
    let mut permissions = fs::metadata(&staged).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&staged, permissions).unwrap();
    fs::rename(staged, &binary).unwrap();

    let state = Arc::new(ServerState::new());
    *state.project_manager.lock() = pkl_project::project_manager::ProjectManager::new(PklCli {
        binary_path: Some(binary),
    });
    let service_state = Arc::clone(&state);
    let (service, socket) =
        LspService::build(move |client| PklBackend::new(Arc::clone(&service_state), Some(client)))
            .custom_method("pkl/syncProjects", PklBackend::sync_projects)
            .finish();
    let (mut client_tx, server_rx) = tokio::io::duplex(16384);
    let (server_tx, mut client_rx) = tokio::io::duplex(16384);
    let server_task = tokio::spawn(async move {
        Server::new(server_rx, server_tx, socket)
            .serve(service)
            .await;
    });

    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "capabilities": {},
            "workspaceFolders": [{
                "uri": url::Url::from_directory_path(&workspace).unwrap().to_string(),
                "name": "workspace"
            }]
        }
    });
    write_message(&mut client_tx, &initialize.to_string()).await;
    let response = tokio::time::timeout(Duration::from_secs(3), read_message(&mut client_rx))
        .await
        .expect("initialize response timed out");
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["id"], 1);
    assert!(response.get("error").is_none(), "{response}");

    write_message(
        &mut client_tx,
        r#"{"jsonrpc":"2.0","id":2,"method":"pkl/syncProjects"}"#,
    )
    .await;
    let response = tokio::time::timeout(Duration::from_secs(3), read_message(&mut client_rx))
        .await
        .expect("syncProjects response timed out");
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["id"], 2);
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(state.project_manager.lock().projects().count(), 1);
    assert!(workspace.join(".lspkl/projects.json").is_file());
    assert_eq!(
        fs::read_to_string(&calls)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["project", "eval"]
    );

    write_message(
        &mut client_tx,
        r#"{"jsonrpc":"2.0","id":3,"method":"shutdown"}"#,
    )
    .await;
    let response = tokio::time::timeout(Duration::from_secs(3), read_message(&mut client_rx))
        .await
        .expect("shutdown response timed out");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response).unwrap()["id"],
        3
    );
    write_message(&mut client_tx, r#"{"jsonrpc":"2.0","method":"exit"}"#).await;
    drop(client_tx);
    tokio::time::timeout(Duration::from_secs(3), server_task)
        .await
        .expect("server did not stop")
        .expect("server task panicked");
}

#[cfg(unix)]
#[tokio::test]
async fn download_package_round_trip_via_jsonrpc() {
    let temp = tempfile::tempdir().unwrap();
    let calls = temp.path().join("download-args.txt");
    let binary = temp.path().join("pkl-stub");
    fs::write(
        &binary,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\nif [ \"$5\" = 'package://example.com/fail@1.0.0' ]; then echo 'test download failure' >&2; exit 7; fi\n",
            shell_quote(&calls.to_string_lossy()),
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&binary).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&binary, permissions).unwrap();
    let cache_dir = temp.path().join("cache");

    let mut state = ServerState::new();
    state.package_cache_dir = Some(cache_dir.clone());
    *state.package_manager.lock() = pkl_project::package_manager::PackageManager::new(PklCli {
        binary_path: Some(binary),
    });
    let state = Arc::new(state);
    let (service, socket) =
        LspService::build(move |client| PklBackend::new(Arc::clone(&state), Some(client)))
            .custom_method("pkl/downloadPackage", PklBackend::download_package)
            .finish();
    let (mut client_tx, server_rx) = tokio::io::duplex(16384);
    let (server_tx, mut client_rx) = tokio::io::duplex(16384);
    let server_task = tokio::spawn(async move {
        Server::new(server_rx, server_tx, socket)
            .serve(service)
            .await;
    });

    write_message(
        &mut client_tx,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#,
    )
    .await;
    let initialized = tokio::time::timeout(Duration::from_secs(3), read_message(&mut client_rx))
        .await
        .expect("initialize response timed out");
    let initialized: serde_json::Value = serde_json::from_str(&initialized).unwrap();
    assert_eq!(initialized["id"], 1);
    assert!(initialized.get("error").is_none(), "{initialized}");

    for (id, uri) in [
        (2, "package://example.com/ok@1.0.0"),
        (3, "not-a-package-uri"),
        (4, "package://example.com/fail@1.0.0"),
    ] {
        let request = serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "pkl/downloadPackage", "params": uri
        });
        write_message(&mut client_tx, &request.to_string()).await;
        let response = tokio::time::timeout(Duration::from_secs(3), read_message(&mut client_rx))
            .await
            .expect("downloadPackage response timed out");
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["id"], id);
        match id {
            2 => {
                assert!(response.get("error").is_none(), "{response}");
                assert_eq!(response["result"], serde_json::Value::Null, "{response}");
            }
            3 => assert_eq!(response["error"]["code"], -32602, "{response}"),
            4 => {
                assert_eq!(response["error"]["code"], -32603, "{response}");
                assert!(
                    response["error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains("test download failure"),
                    "{response}"
                );
            }
            _ => unreachable!(),
        }
    }
    assert_eq!(
        fs::read_to_string(&calls)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [
            "download-package",
            "--no-transitive",
            "--cache-dir",
            cache_dir.to_str().unwrap(),
            "package://example.com/ok@1.0.0",
            "download-package",
            "--no-transitive",
            "--cache-dir",
            cache_dir.to_str().unwrap(),
            "package://example.com/fail@1.0.0",
        ]
    );
    write_message(
        &mut client_tx,
        r#"{"jsonrpc":"2.0","id":5,"method":"shutdown"}"#,
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(3), read_message(&mut client_rx))
        .await
        .expect("shutdown response timed out");
    write_message(&mut client_tx, r#"{"jsonrpc":"2.0","method":"exit"}"#).await;
    drop(client_tx);
    tokio::time::timeout(Duration::from_secs(3), server_task)
        .await
        .expect("server did not stop")
        .expect("server task panicked");
}

#[cfg(unix)]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Write one LSP-framed message: `Content-Length: <n>\r\n\r\n<body>`.
async fn write_message<W: AsyncWriteExt + Unpin>(writer: &mut W, body: &str) {
    let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
    writer.write_all(frame.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();
}

/// Read one LSP-framed message and return its JSON body as a String.
///
/// Reads byte-by-byte until `\r\n\r\n`, parses `Content-Length`, then reads
/// exactly that many bytes. Inefficient but trivially correct — fine for a
/// single smoke test.
async fn read_message<R: AsyncReadExt + Unpin>(reader: &mut R) -> String {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        reader.read_exact(&mut byte).await.expect("header read");
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }

    let header_str = std::str::from_utf8(&header).expect("header utf8");
    let content_length: usize = header_str
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length:"))
        .map(|s| s.trim().parse().expect("Content-Length must parse"))
        .expect("Content-Length header missing");

    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).await.expect("body read");
    String::from_utf8(body).expect("body utf8")
}
