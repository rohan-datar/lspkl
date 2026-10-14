//! Lifecycle tests: initialize → didOpen → didChange → didClose → shutdown.

use lsp_types::*;
use lspkl::{Workspace, handlers};

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

#[test]
fn initialize_sets_file_workspace_folders() {
    let mut backend = Workspace::new();
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
    initialize_workspace(&mut backend, params).unwrap();
    assert_eq!(
        backend.project_manager().workspace_folders(),
        [workspace.path().to_path_buf()]
    );
}

#[test]
fn initialize_falls_back_to_root_uri() {
    let mut backend = Workspace::new();
    let workspace = tempfile::tempdir().unwrap();
    let workspace_uri = url::Url::from_directory_path(workspace.path()).unwrap();
    let mut params = InitializeParams::default();
    #[allow(deprecated)]
    {
        params.root_uri = Some(workspace_uri.as_str().parse().unwrap());
    }
    initialize_workspace(&mut backend, params).unwrap();
    assert_eq!(
        backend.project_manager().workspace_folders(),
        [workspace.path().to_path_buf()]
    );
}

#[test]
fn did_open_then_state_has_file() {
    let mut backend = Workspace::new();
    let _ = initialize_workspace(&mut backend, InitializeParams::default()).unwrap();

    let uri = pkl_url("a.pkl");
    handlers::lifecycle::did_open(&mut backend, open_params(uri.clone(), "foo = 1", 1)).unwrap();

    let snapshot = backend.snapshot();
    let file = snapshot
        .document(uri.as_str())
        .expect("file should be open after did_open");
    assert_eq!(file.text(&snapshot.db).as_ref(), "foo = 1");
    assert_eq!(file.version(&snapshot.db), Some(1));
    assert_eq!(file.status(&snapshot.db), pkl_db::FileStatus::Open);
}

#[test]
fn did_change_updates_contents() {
    let mut backend = Workspace::new();
    let _ = initialize_workspace(&mut backend, InitializeParams::default()).unwrap();

    let uri = pkl_url("b.pkl");
    handlers::lifecycle::did_open(&mut backend, open_params(uri.clone(), "foo = 1", 1)).unwrap();

    handlers::lifecycle::did_change(
        &mut backend,
        DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "foo = 2".into(),
            }],
        },
    )
    .unwrap();

    let snapshot = backend.snapshot();
    let file = snapshot.document(uri.as_str()).unwrap();
    assert_eq!(file.text(&snapshot.db).as_ref(), "foo = 2");
    assert_eq!(file.version(&snapshot.db), Some(2));
}

#[test]
fn did_close_removes_file_from_state() {
    let mut backend = Workspace::new();
    let _ = initialize_workspace(&mut backend, InitializeParams::default()).unwrap();

    let uri = pkl_url("c.pkl");
    handlers::lifecycle::did_open(&mut backend, open_params(uri.clone(), "foo = 1", 1)).unwrap();

    {
        let work = handlers::lifecycle::did_close(
            &mut backend,
            DidCloseTextDocumentParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
            },
        )
        .unwrap();
        backend.apply_reload(work.load());
    };

    assert!(
        backend.document(uri.as_str()).is_none(),
        "file must no longer be open after did_close"
    );
}

#[test]
fn initialization_without_settings_preserves_project_discovery_exclusions() {
    let temp = tempfile::tempdir().unwrap();
    for directory in ["node_modules", "target", "src"] {
        std::fs::create_dir(temp.path().join(directory)).unwrap();
        std::fs::write(temp.path().join(directory).join("PklProject"), "").unwrap();
    }
    let mut backend = Workspace::new();
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
    initialize_workspace(&mut backend, params.clone()).unwrap();
    assert_eq!(
        backend.project_manager().discover_project_files().unwrap(),
        [temp.path().join("src/PklProject")]
    );
    let params = InitializeParams {
        initialization_options: Some(
            serde_json::json!({"pkl":{"projects":{"excludedDirectories":[]}}}),
        ),
        ..params
    };
    initialize_workspace(&mut backend, params).unwrap();
    assert_eq!(
        backend.project_manager().discover_project_files().unwrap(),
        [
            "node_modules/PklProject",
            "src/PklProject",
            "target/PklProject"
        ]
        .map(|path| temp.path().join(path))
    );
}

fn initialize_workspace(
    workspace: &mut Workspace,
    params: InitializeParams,
) -> handlers::Result<InitializeResult> {
    let result = handlers::lifecycle::initialize(workspace, &params);
    let mut settings = workspace.settings().clone();
    settings.resolve_paths(workspace.project_manager().workspace_folders());
    if params.initialization_options.is_some() {
        workspace.apply_settings(settings);
    }
    let manager = handlers::lifecycle::restore_workspaces(workspace.project_manager())?;
    workspace.apply_project_manager(manager);
    Ok(result)
}
