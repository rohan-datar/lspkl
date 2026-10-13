//! Lifecycle tests: initialize → didOpen → didChange → didClose → shutdown.

use lsp_types::*;
use lspkl::PklBackend;
use tower_lsp_server::LanguageServer;

fn pkl_url(name: &str) -> Uri {
    format!("inmemory:///{name}").parse().unwrap()
}

fn open_params(uri: Uri, contents: &str, version: i32) -> DidOpenTextDocumentParams {
    DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri,
            language_id: "pkl".into(),
            version,
            text: contents.into(),
        },
    }
}

#[tokio::test]
async fn initialize_sets_file_workspace_folders() {
    let backend = PklBackend::for_testing();
    let workspace = tempfile::Builder::new()
        .prefix("lspkl workspace ")
        .tempdir()
        .unwrap();
    let workspace_uri = url::Url::from_directory_path(workspace.path()).unwrap();
    let params = InitializeParams {
        workspace_folders: Some(vec![
            WorkspaceFolder {
                uri: workspace_uri.as_str().parse().unwrap(),
                name: "project".into(),
            },
            WorkspaceFolder {
                uri: "https://example.com/not-local".parse().unwrap(),
                name: "remote".into(),
            },
        ]),
        ..InitializeParams::default()
    };
    backend.initialize(params).await.unwrap();
    assert_eq!(
        backend
            .workspace
            .lock()
            .snapshot()
            .project_manager
            .lock()
            .workspace_folders(),
        [workspace.path().to_path_buf()]
    );
}

#[tokio::test]
async fn initialize_falls_back_to_root_uri() {
    let backend = PklBackend::for_testing();
    let workspace = tempfile::tempdir().unwrap();
    let workspace_uri = url::Url::from_directory_path(workspace.path()).unwrap();
    let mut params = InitializeParams::default();
    #[allow(deprecated)]
    {
        params.root_uri = Some(workspace_uri.as_str().parse().unwrap());
    }
    backend.initialize(params).await.unwrap();
    assert_eq!(
        backend
            .workspace
            .lock()
            .snapshot()
            .project_manager
            .lock()
            .workspace_folders(),
        [workspace.path().to_path_buf()]
    );
}

#[tokio::test]
async fn did_open_then_state_has_file() {
    let backend = PklBackend::for_testing();
    let _ = backend
        .initialize(InitializeParams::default())
        .await
        .unwrap();

    let uri = pkl_url("a.pkl");
    backend
        .did_open(open_params(uri.clone(), "foo = 1", 1))
        .await;

    let snapshot = backend.workspace.lock().snapshot();
    let file = snapshot
        .document(uri.as_str())
        .expect("file should be open after did_open");
    assert_eq!(file.text(&snapshot.db).as_ref(), "foo = 1");
    assert_eq!(file.version(&snapshot.db), Some(1));
    assert_eq!(file.status(&snapshot.db), pkl_db::FileStatus::Open);
}

#[tokio::test]
async fn did_change_updates_contents() {
    let backend = PklBackend::for_testing();
    let _ = backend
        .initialize(InitializeParams::default())
        .await
        .unwrap();

    let uri = pkl_url("b.pkl");
    backend
        .did_open(open_params(uri.clone(), "foo = 1", 1))
        .await;

    backend
        .did_change(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "foo = 2".into(),
            }],
        })
        .await;

    let snapshot = backend.workspace.lock().snapshot();
    let file = snapshot.document(uri.as_str()).unwrap();
    assert_eq!(file.text(&snapshot.db).as_ref(), "foo = 2");
    assert_eq!(file.version(&snapshot.db), Some(2));
}

#[tokio::test]
async fn did_close_removes_file_from_state() {
    let backend = PklBackend::for_testing();
    let _ = backend
        .initialize(InitializeParams::default())
        .await
        .unwrap();

    let uri = pkl_url("c.pkl");
    backend
        .did_open(open_params(uri.clone(), "foo = 1", 1))
        .await;

    backend
        .did_close(DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
        })
        .await;

    assert!(
        backend.workspace.lock().document(uri.as_str()).is_none(),
        "file must no longer be open after did_close"
    );
}

#[tokio::test]
async fn shutdown_succeeds() {
    let backend = PklBackend::for_testing();
    let _ = backend
        .initialize(InitializeParams::default())
        .await
        .unwrap();
    backend.shutdown().await.expect("shutdown should succeed");
}

#[tokio::test]
async fn initialization_without_settings_preserves_project_discovery_exclusions() {
    let temp = tempfile::tempdir().unwrap();
    for directory in ["node_modules", "target", "src"] {
        std::fs::create_dir(temp.path().join(directory)).unwrap();
        std::fs::write(temp.path().join(directory).join("PklProject"), "").unwrap();
    }
    let backend = PklBackend::for_testing();
    let params = InitializeParams {
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: pkl_vfs::path_to_uri(temp.path())
                .unwrap()
                .as_str()
                .parse()
                .unwrap(),
            name: "fixture".into(),
        }]),
        ..Default::default()
    };
    backend.initialize(params.clone()).await.unwrap();
    let snapshot = backend.workspace.lock().snapshot();
    assert_eq!(
        snapshot
            .project_manager
            .lock()
            .discover_project_files()
            .unwrap(),
        [temp.path().join("src/PklProject")]
    );
    drop(snapshot);
    let params = InitializeParams {
        initialization_options: Some(
            serde_json::json!({"pkl":{"projects":{"excludedDirectories":[]}}}),
        ),
        ..params
    };
    backend.initialize(params).await.unwrap();
    let snapshot = backend.workspace.lock().snapshot();
    assert_eq!(
        snapshot
            .project_manager
            .lock()
            .discover_project_files()
            .unwrap(),
        [
            "node_modules/PklProject",
            "src/PklProject",
            "target/PklProject"
        ]
        .map(|path| temp.path().join(path))
    );
}

#[tokio::test]
async fn dropping_backend_releases_workspace_held_by_diagnostic_workers() {
    let workspace = std::sync::Arc::new(parking_lot::Mutex::new(lspkl::Workspace::new()));
    let retained = std::sync::Arc::downgrade(&workspace);
    let (service, socket) =
        tower_lsp_server::LspService::new(move |client| PklBackend::new(workspace, Some(client)));
    service
        .inner()
        .did_open(open_params(pkl_url("teardown.pkl"), "value = 1", 1))
        .await;
    tokio::task::yield_now().await;
    drop(service);
    drop(socket);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while retained.upgrade().is_some() {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("closed diagnostic workers must release their workspace");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backend_request_cancellation_returns_content_modified_and_next_request_is_fresh() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let armed = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let callback_armed = armed.clone();
    let callback_entered = entered.clone();
    let callback_release = release.clone();
    let db = pkl_db::RootDatabase::with_event_callback(
        Arc::new(pkl_vfs::StandardSourceLoader::default()),
        Default::default(),
        Some(Box::new(move |event| match event.kind {
            salsa::EventKind::WillExecute { .. }
                if callback_armed.swap(false, Ordering::SeqCst) =>
            {
                callback_entered.notify_one();
                let (lock, condition) = &*callback_release;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = condition.wait(released).unwrap();
                }
            }
            salsa::EventKind::DidSetCancellationFlag => {
                let (lock, condition) = &*callback_release;
                *lock.lock().unwrap() = true;
                condition.notify_all();
            }
            _ => {}
        })),
    );
    let workspace = Arc::new(parking_lot::Mutex::new(lspkl::Workspace::with_database(db)));
    let backend = Arc::new(PklBackend::new(workspace, None));
    let uri = pkl_url("cancel.pkl");
    backend
        .did_open(open_params(
            uri.clone(),
            "value: Int = 1\nanswer = value\n",
            1,
        ))
        .await;
    *release.0.lock().unwrap() = false;
    armed.store(true, Ordering::SeqCst);
    let params = HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: Position::new(1, 10),
        },
        work_done_progress_params: Default::default(),
    };
    let request_backend = backend.clone();
    let request_params = params.clone();
    let request = tokio::spawn(async move { request_backend.hover(request_params).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .expect("worker must reach WillExecute");
    let writer = backend.clone();
    let write_uri = uri.clone();
    tokio::task::spawn_blocking(move || {
        writer
            .workspace
            .lock()
            .apply_change(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: write_uri,
                    version: 2,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: "value: String = \"new\"\nanswer = value\n".into(),
                }],
            })
    })
    .await
    .unwrap()
    .unwrap();
    let error = tokio::time::timeout(std::time::Duration::from_secs(5), request)
        .await
        .unwrap()
        .expect("request must return a protocol error, not panic")
        .unwrap_err();
    assert_eq!(serde_json::to_value(error).unwrap()["code"], -32801);
    let hover = backend.hover(params).await.unwrap().unwrap();
    let HoverContents::Markup(contents) = hover.contents else {
        panic!("expected markup")
    };
    assert!(
        contents.value.contains("value: String"),
        "{}",
        contents.value
    );
}
