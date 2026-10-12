//! AST-to-file import resolution using real syntax trees and temporary workspaces.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use fluent_uri::Uri;
use lspkl::Workspace;
use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};
use pkl_vfs::VirtualFile;

fn file_uri(path: &Path) -> Uri<String> {
    Uri::parse(url::Url::from_file_path(path).unwrap().to_string()).unwrap()
}

fn source(state: &mut Workspace, path: &Path, text: &str) -> (Arc<VirtualFile>, Arc<SyntaxTree>) {
    let file = open(state, file_uri(path), text.into(), Some(1));
    let tree = state.snapshot().file_manager.parse(&file.url).unwrap();
    (file, tree)
}

fn import_node(tree: &SyntaxTree) -> NodeId {
    tree.child(tree.root(), |kind| matches!(kind, NodeKind::Import))
        .expect("fixture must contain an ordinary import")
}

fn load_projects(state: &mut Workspace, workspace: &Path, projects: &[(&str, &[&str])]) {
    let entries: Vec<_> = projects
        .iter()
        .map(|(project_file, roots)| {
            serde_json::json!({
                "projectFile": project_file,
                "packageUri": null,
                "declaredDependencies": {},
                "resolvedDependencies": null,
                "evaluatorSettings": { "moduleCacheDir": null, "modulePath": roots }
            })
        })
        .collect();
    let state_file = workspace.join(".lspkl/projects.json");
    fs::create_dir_all(state_file.parent().unwrap()).unwrap();
    fs::write(
        &state_file,
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "pklProjects": entries
        }))
        .unwrap(),
    )
    .unwrap();
    state.apply_initialize(&lsp_types::InitializeParams {
        workspace_folders: Some(vec![lsp_types::WorkspaceFolder {
            uri: file_uri(workspace).as_str().parse().unwrap(),
            name: "fixture".into(),
        }]),
        ..Default::default()
    });
    let view = state.snapshot();
    let mut manager = view.project_manager.lock();
    manager.load_state(&state_file).unwrap();
}

#[test]
fn relative_import_uses_ast_literal_and_source_directory_without_a_project() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("source directory");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("library.pkl"), "answer = 42\n").unwrap();
    let mut state = Workspace::new();
    let (source, tree) = source(
        &mut state,
        &dir.join("main.pkl"),
        "// import \"./not-the-target.pkl\"\nimport \"./library.pkl\"\n",
    );

    let target = state
        .snapshot()
        .resolve_import_file(&tree, import_node(&tree), &source)
        .unwrap();

    assert_eq!(target.url, file_uri(&dir.join("library.pkl")));
    assert_eq!(target.contents(), "answer = 42\n");
}

#[test]
fn modulepath_import_selects_the_nearest_project_for_each_source() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path();
    fs::create_dir_all(workspace.join("outer-lib")).unwrap();
    fs::create_dir_all(workspace.join("app/inner-lib")).unwrap();
    fs::write(
        workspace.join("outer-lib/library.pkl"),
        "owner = \"outer\"\n",
    )
    .unwrap();
    fs::write(
        workspace.join("app/inner-lib/library.pkl"),
        "owner = \"inner\"\n",
    )
    .unwrap();
    let mut state = Workspace::new();
    load_projects(
        &mut state,
        workspace,
        &[
            ("PklProject", &["outer-lib"]),
            ("app/PklProject", &["inner-lib"]),
        ],
    );
    let (outer, outer_tree) = source(
        &mut state,
        &workspace.join("main.pkl"),
        "import \"modulepath:/library.pkl\"\n",
    );
    let (inner, inner_tree) = source(
        &mut state,
        &workspace.join("app/main.pkl"),
        "import \"modulepath:/library.pkl\"\n",
    );

    let outer_target = state
        .snapshot()
        .resolve_import_file(&outer_tree, import_node(&outer_tree), &outer)
        .unwrap();
    let inner_target = state
        .snapshot()
        .resolve_import_file(&inner_tree, import_node(&inner_tree), &inner)
        .unwrap();

    assert_eq!(
        outer_target.url,
        file_uri(&workspace.join("outer-lib/library.pkl"))
    );
    assert_eq!(
        inner_target.url,
        file_uri(&workspace.join("app/inner-lib/library.pkl"))
    );
    assert_eq!(outer_target.contents(), "owner = \"outer\"\n");
    assert_eq!(inner_target.contents(), "owner = \"inner\"\n");
}

#[test]
fn imported_file_reuses_unsaved_editor_contents() {
    let temp = tempfile::tempdir().unwrap();
    let target_path = temp.path().join("library.pkl");
    fs::write(&target_path, "answer = 0\n").unwrap();
    let mut state = Workspace::new();
    let opened = open(
        &mut state,
        file_uri(&target_path),
        "answer = 42\n".into(),
        Some(2),
    );
    let (source, tree) = source(
        &mut state,
        &temp.path().join("main.pkl"),
        "import \"./library.pkl\"\n",
    );

    let target = state
        .snapshot()
        .resolve_import_file(&tree, import_node(&tree), &source)
        .unwrap();

    assert!(Arc::ptr_eq(&target, &opened));
    assert_eq!(target.contents(), "answer = 42\n");
    assert_eq!(target.version(), Some(2));
}

#[test]
fn missing_or_unsupported_import_targets_return_none() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = Workspace::new();
    for uri in [
        "./missing.pkl",
        "modulepath:/library.pkl",
        "ftp://example.com/library.pkl",
    ] {
        let (source, tree) = source(
            &mut state,
            &temp.path().join("main.pkl"),
            &format!("import \"{uri}\"\n"),
        );
        assert!(
            state
                .snapshot()
                .resolve_import_file(&tree, import_node(&tree), &source)
                .is_none(),
            "unsupported or unresolved import: {uri}"
        );
    }
}

#[test]
fn glob_imports_and_non_import_nodes_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("library.pkl"), "answer = 42\n").unwrap();
    let mut state = Workspace::new();
    let (source, tree) = source(
        &mut state,
        &temp.path().join("main.pkl"),
        "import* \"./library.pkl\"\nanswer = 1\n",
    );
    let glob = tree
        .child(tree.root(), |kind| matches!(kind, NodeKind::ImportGlob))
        .expect("fixture must contain a glob import");
    let property = tree
        .child(tree.root(), |kind| matches!(kind, NodeKind::ClassProperty))
        .unwrap();

    assert!(
        state
            .snapshot()
            .resolve_import_file(&tree, glob, &source)
            .is_none()
    );
    assert!(
        state
            .snapshot()
            .resolve_import_file(&tree, property, &source)
            .is_none()
    );
}

fn open(
    state: &mut Workspace,
    url: Uri<String>,
    text: String,
    version: Option<i32>,
) -> Arc<pkl_vfs::VirtualFile> {
    state
        .apply_open(lsp_types::DidOpenTextDocumentParams {
            text_document: lsp_types::TextDocumentItem {
                uri: url.as_str().parse().unwrap(),
                language_id: "pkl".into(),
                version: version.unwrap_or(1),
                text,
            },
        })
        .unwrap();
    state.snapshot().document(url.as_str()).unwrap()
}
