//! Imported-module parsing contracts, including editor-update snapshot behavior.
//! Cross-file analysis and live LSP handler integration come later.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use fluent_uri::Uri;
use lspkl::ServerState;
use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};
use pkl_vfs::VirtualFile;

fn file_uri(path: &Path) -> Uri<String> {
    Uri::parse(url::Url::from_file_path(path).unwrap().to_string()).unwrap()
}

fn importing_source(
    state: &ServerState,
    dir: &Path,
) -> (Arc<VirtualFile>, Arc<SyntaxTree>, NodeId) {
    let file = state.file_manager.open(
        file_uri(&dir.join("main.pkl")),
        "import \"./library.pkl\"\n".into(),
        Some(1),
    );
    let tree = state.file_manager.parse(&file.url).unwrap();
    let import = tree
        .child(tree.root(), |kind| matches!(kind, NodeKind::Import))
        .unwrap();
    (file, tree, import)
}

fn property_name(tree: &SyntaxTree) -> &str {
    let property = tree
        .child(tree.root(), |kind| matches!(kind, NodeKind::ClassProperty))
        .expect("imported tree must contain its property's declaration");
    tree.name(property).unwrap()
}

#[test]
fn resolves_and_parses_the_target_not_the_importing_tree() {
    let temp = tempfile::tempdir().unwrap();
    let target_path = temp.path().join("library.pkl");
    fs::write(&target_path, "answer = 42\n").unwrap();
    let state = ServerState::new();
    let (source, tree, import) = importing_source(&state, temp.path());

    let module = state
        .resolve_import_module(&tree, import, &source)
        .unwrap()
        .unwrap();

    assert_eq!(module.file.url, file_uri(&target_path));
    assert_eq!(property_name(&module.tree), "answer");
    assert_eq!(module.tree.source(), "answer = 42\n");
}

#[test]
fn editor_changes_refresh_imported_tree_without_mutating_previous_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let target_path = temp.path().join("library.pkl");
    fs::write(&target_path, "diskValue = 0\n").unwrap();
    let state = ServerState::new();
    let opened = state
        .file_manager
        .open(file_uri(&target_path), "oldValue = 1\n".into(), Some(1));
    let (source, tree, import) = importing_source(&state, temp.path());
    let before = state
        .resolve_import_module(&tree, import, &source)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&before.file, &opened));
    assert_eq!(property_name(&before.tree), "oldValue");

    state
        .file_manager
        .update(&opened.url, "newValue = 2\n".into(), Some(2))
        .unwrap();
    let after = state
        .resolve_import_module(&tree, import, &source)
        .unwrap()
        .unwrap();

    assert!(Arc::ptr_eq(&after.file, &opened));
    assert_eq!(property_name(&after.tree), "newValue");
    assert_eq!(after.tree.source(), "newValue = 2\n");
    assert_eq!(property_name(&before.tree), "oldValue");
    assert_eq!(before.tree.source(), "oldValue = 1\n");
}

#[test]
fn missing_target_is_unresolved_rather_than_a_parse_error() {
    let temp = tempfile::tempdir().unwrap();
    let state = ServerState::new();
    let (source, tree, import) = importing_source(&state, temp.path());

    assert!(
        state
            .resolve_import_module(&tree, import, &source)
            .expect("unresolved import is not a parsing error")
            .is_none()
    );
}
