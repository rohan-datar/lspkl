mod support;

use std::{fs, path::Path};

use fluent_uri::Uri;
use lspkl::Workspace;
use lspkl_db::{Db, File, ImportResolution, parse, resolve_import};
use lspkl_syntax::ast::NodeKind;
use serde_json::json;

const DEPENDENCY: &str = "package://example.com/library@1.0.0";

fn uri(path: &Path) -> Uri<String> {
    lspkl_vfs::path_to_uri(path).unwrap()
}

fn open(state: &mut Workspace, path: &Path, text: &str) -> File {
    let url = uri(path);
    state
        .apply_open(lsp_types::DidOpenTextDocumentParams {
            text_document: lsp_types::TextDocumentItem {
                uri: url.as_str().parse().unwrap(),
                language_id: "pkl".into(),
                version: 1,
                text: text.into(),
            },
        })
        .unwrap();
    support::preload(state);
    state.document(url.as_str()).unwrap()
}

fn resolved_dependencies(path: &str) -> serde_json::Value {
    json!({DEPENDENCY: {"type": "local", "uri": DEPENDENCY, "path": path}})
}

fn fixture(root: &Path) -> Workspace {
    for directory in ["lib", "dependency", "replacement"] {
        fs::create_dir(root.join(directory)).unwrap();
        fs::write(root.join(directory).join("library.pkl"), "answer = 42\n").unwrap();
    }
    fs::write(root.join("PklProject"), "amends \"pkl:Project\"\n").unwrap();
    fs::write(
        root.join("PklProject.deps.json"),
        json!({"schemaVersion": 1, "resolvedDependencies": resolved_dependencies("dependency")})
            .to_string(),
    )
    .unwrap();
    let state_file = root.join(".lspkl/projects.json");
    fs::create_dir(state_file.parent().unwrap()).unwrap();
    fs::write(
        &state_file,
        json!({
            "schemaVersion": 1,
            "pklProjects": [{
                "projectFile": "PklProject",
                "packageUri": null,
                "declaredDependencies": {"library": DEPENDENCY},
                "resolvedDependencies": resolved_dependencies("dependency"),
                "evaluatorSettings": {"moduleCacheDir": null, "modulePath": ["lib"]}
            }]
        })
        .to_string(),
    )
    .unwrap();
    let mut state = Workspace::new();
    state.apply_initialize(&lsp_types::InitializeParams {
        workspace_folders: Some(vec![lsp_types::WorkspaceFolder {
            uri: uri(root).as_str().parse().unwrap(),
            name: "fixture".into(),
        }]),
        ..Default::default()
    });
    let mut manager = state.project_manager();
    manager.load_state(&state_file).unwrap();
    state.apply_project_manager(manager);
    state
        .database()
        .file(&uri(&root.join("PklProject")))
        .unwrap();
    state
        .database()
        .file(&uri(&root.join("PklProject.deps.json")))
        .unwrap();
    state
}

fn assert_route(state: &mut Workspace, root: &Path, reference: &str, target: &str) {
    let source = open(
        state,
        &root.join("main.pkl"),
        &format!("import \"{reference}\"\n"),
    );
    let tree = parse(state.database(), source).as_ref().unwrap().clone();
    let import = tree
        .child(tree.root(), |kind| matches!(kind, NodeKind::Import))
        .unwrap();
    let ImportResolution::Single(file) = resolve_import(state.database(), source, import, None)
    else {
        panic!("project import {reference} must resolve");
    };
    assert_eq!(file.uri(state.database()), &uri(&root.join(target)));
}

fn assert_original_routes(state: &mut Workspace, root: &Path) {
    assert_route(state, root, "modulepath:/library.pkl", "lib/library.pkl");
    assert_route(
        state,
        root,
        "@library/library.pkl",
        "dependency/library.pkl",
    );
}

fn watched(state: &mut Workspace, path: &Path) {
    let work = state.prepare_watched_files(lsp_types::DidChangeWatchedFilesParams {
        changes: vec![lsp_types::FileEvent {
            uri: uri(path).as_str().parse().unwrap(),
            typ: lsp_types::FileChangeType::CHANGED,
        }],
    });
    state.apply_reload(work.load());
}

#[test]
fn closing_unchanged_project_preserves_restored_import_routes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut state = fixture(root);
    assert_original_routes(&mut state, root);
    let project = root.join("PklProject");
    open(&mut state, &project, &fs::read_to_string(&project).unwrap());
    let work = state
        .apply_close(lsp_types::DidCloseTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier {
                uri: uri(&project).as_str().parse().unwrap(),
            },
        })
        .unwrap();
    state.apply_reload(work.load());
    assert_original_routes(&mut state, root);
}

#[test]
fn full_refresh_preserves_restored_import_routes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut state = fixture(root);
    assert_original_routes(&mut state, root);
    let work = state.prepare_refresh();
    state.apply_reload(work.load());
    assert_original_routes(&mut state, root);
}

#[test]
fn watched_project_edit_invalidates_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut state = fixture(root);
    assert_original_routes(&mut state, root);
    fs::write(
        root.join("PklProject"),
        "amends \"pkl:Project\"\n// changed\n",
    )
    .unwrap();
    watched(&mut state, &root.join("PklProject"));
    assert!(
        state
            .project_manager()
            .get_pkl_project(&root.join("main.pkl"))
            .is_none()
    );
    let source = state
        .document(uri(&root.join("main.pkl")).as_str())
        .unwrap();
    let tree = parse(state.database(), source).as_ref().unwrap().clone();
    let import = tree
        .child(tree.root(), |kind| matches!(kind, NodeKind::Import))
        .unwrap();
    assert!(matches!(
        resolve_import(state.database(), source, import, None),
        ImportResolution::Unresolved { .. }
    ));
}

#[test]
fn watched_dependency_file_updates_routing_without_losing_modulepath() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut state = fixture(root);
    assert_original_routes(&mut state, root);
    fs::write(
        root.join("PklProject.deps.json"),
        json!({"schemaVersion": 1, "resolvedDependencies": resolved_dependencies("replacement")})
            .to_string(),
    )
    .unwrap();
    watched(&mut state, &root.join("PklProject.deps.json"));
    assert_route(
        &mut state,
        root,
        "modulepath:/library.pkl",
        "lib/library.pkl",
    );
    assert_route(
        &mut state,
        root,
        "@library/library.pkl",
        "replacement/library.pkl",
    );
}

#[cfg(unix)]
#[test]
fn full_refresh_after_successful_sync_preserves_import_routes() {
    use lspkl_project::{cli::PklCli, project_manager::ProjectManager};
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut state = fixture(root);
    let metadata = json!({
        "projectFileUri": uri(&root.join("PklProject")).as_str(),
        "declaredDependencies": {"library": DEPENDENCY},
        "evaluatorSettings": {"modulePath": ["lib"]}
    });
    let script = root.join("fake-pkl");
    let quoted = format!("'{}'", metadata.to_string().replace('\'', "'\\''"));
    fs::write(&script, format!("#!/bin/sh\ncase \"$1\" in\nproject) exit 0 ;;\neval) printf '%s' {quoted} ;;\n*) exit 9 ;;\nesac\n")).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let mut manager = ProjectManager::new(PklCli {
        binary_path: Some(script),
    });
    manager.initialize(vec![root.to_path_buf()]);
    manager.sync_projects().unwrap();
    state.apply_project_manager(manager);
    assert_original_routes(&mut state, root);
    let work = state.prepare_refresh();
    state.apply_reload(work.load());
    assert_original_routes(&mut state, root);
}

#[test]
fn later_notifications_preserve_watched_project_intent() {
    for name in ["PklProject", "PklProject.deps.json"] {
        for notification in ["refresh", "close", "open", "change"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let mut state = fixture(root);
            let path = root.join(name);
            if matches!(notification, "close" | "change") {
                open(&mut state, &path, &fs::read_to_string(&path).unwrap());
            }
            let changed = if name == "PklProject" {
                "amends \"pkl:Project\"\n// changed\n".to_owned()
            } else {
                json!({"schemaVersion": 1, "resolvedDependencies": resolved_dependencies("replacement")}).to_string()
            };
            fs::write(&path, changed).unwrap();
            let watched = state.prepare_watched_files(lsp_types::DidChangeWatchedFilesParams {
                changes: vec![lsp_types::FileEvent {
                    uri: uri(&path).as_str().parse().unwrap(),
                    typ: lsp_types::FileChangeType::CHANGED,
                }],
            });
            let newer = match notification {
                "close" => Some(
                    state
                        .apply_close(lsp_types::DidCloseTextDocumentParams {
                            text_document: lsp_types::TextDocumentIdentifier {
                                uri: uri(&path).as_str().parse().unwrap(),
                            },
                        })
                        .unwrap(),
                ),
                "refresh" => Some(state.prepare_refresh()),
                "open" => {
                    open(&mut state, &path, "editorOnly = 42");
                    None
                }
                "change" => {
                    state
                        .apply_change(lsp_types::DidChangeTextDocumentParams {
                            text_document: lsp_types::VersionedTextDocumentIdentifier {
                                uri: uri(&path).as_str().parse().unwrap(),
                                version: 2,
                            },
                            content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                                range: None,
                                range_length: None,
                                text: "editorOnly = 42".into(),
                            }],
                        })
                        .unwrap();
                    None
                }
                _ => unreachable!(),
            };
            state.apply_reload(watched.load());
            if let Some(newer) = newer {
                state.apply_reload(newer.load());
            }
            if matches!(notification, "open" | "change") {
                let file = state.document(uri(&path).as_str()).unwrap();
                assert_eq!(
                    file.text(state.database()).as_ref(),
                    "editorOnly = 42",
                    "watched metadata must not replace editor text"
                );
            }
            if name == "PklProject" {
                let file = open(
                    &mut state,
                    &root.join("main.pkl"),
                    "import \"modulepath:/library.pkl\"",
                );
                let tree = parse(state.database(), file).as_ref().unwrap();
                let owner = tree
                    .child(tree.root(), |kind| matches!(kind, NodeKind::Import))
                    .unwrap();
                assert!(
                    matches!(
                        resolve_import(state.database(), file, owner, None),
                        ImportResolution::Unresolved { .. }
                    ),
                    "watched project invalidation must survive a superseding ordinary reload"
                );
            } else {
                assert_route(
                    &mut state,
                    root,
                    "@library/library.pkl",
                    "replacement/library.pkl",
                );
                assert_route(
                    &mut state,
                    root,
                    "modulepath:/library.pkl",
                    "lib/library.pkl",
                );
            }
        }
    }
}
