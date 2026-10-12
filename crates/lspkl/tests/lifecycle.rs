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

    // state.file_manager should now know about the URL.
    let url = fluent_uri::Uri::parse(uri.as_str().to_string()).unwrap();
    let file = backend
        .workspace
        .lock()
        .snapshot()
        .file_manager
        .get(&url)
        .expect("file should be registered after did_open");
    assert_eq!(file.contents(), "foo = 1");
    assert_eq!(file.version(), Some(1));
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

    let url = fluent_uri::Uri::parse(uri.as_str().to_string()).unwrap();
    let file = backend
        .workspace
        .lock()
        .snapshot()
        .file_manager
        .get(&url)
        .unwrap();
    assert_eq!(file.contents(), "foo = 2");
    assert_eq!(file.version(), Some(2));
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

    let url = fluent_uri::Uri::parse(uri.as_str().to_string()).unwrap();
    assert!(
        backend
            .workspace
            .lock()
            .snapshot()
            .file_manager
            .get(&url)
            .is_none(),
        "file must be removed after did_close"
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
    let params = InitializeParams {
        initialization_options: Some(
            serde_json::json!({"pkl":{"projects":{"excludedDirectories":[]}}}),
        ),
        ..params
    };
    backend.initialize(params).await.unwrap();
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
