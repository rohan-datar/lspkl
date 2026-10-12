//! Imported-module parsing contracts, including editor-update snapshot behavior.

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

fn importing_source(
    state: &mut Workspace,
    dir: &Path,
) -> (Arc<VirtualFile>, Arc<SyntaxTree>, NodeId) {
    let file = open(
        state,
        file_uri(&dir.join("main.pkl")),
        "import \"./library.pkl\"\n".into(),
        Some(1),
    );
    let tree = state.snapshot().file_manager.parse(&file.url).unwrap();
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
    let mut state = Workspace::new();
    let (source, tree, import) = importing_source(&mut state, temp.path());

    let module = state
        .snapshot()
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
    let mut state = Workspace::new();
    let opened = open(
        &mut state,
        file_uri(&target_path),
        "oldValue = 1\n".into(),
        Some(1),
    );
    let (source, tree, import) = importing_source(&mut state, temp.path());
    let before = state
        .snapshot()
        .resolve_import_module(&tree, import, &source)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&before.file, &opened));
    assert_eq!(property_name(&before.tree), "oldValue");

    change(&mut state, &opened.url, "newValue = 2\n", 2);
    let after = state
        .snapshot()
        .resolve_import_module(&tree, import, &source)
        .unwrap()
        .unwrap();

    assert_eq!(after.file.version(), Some(2));
    assert_eq!(property_name(&after.tree), "newValue");
    assert_eq!(after.tree.source(), "newValue = 2\n");
    assert_eq!(property_name(&before.tree), "oldValue");
    assert_eq!(before.tree.source(), "oldValue = 1\n");
}

#[test]
fn missing_target_is_unresolved_rather_than_a_parse_error() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = Workspace::new();
    let (source, tree, import) = importing_source(&mut state, temp.path());

    assert!(
        state
            .snapshot()
            .resolve_import_module(&tree, import, &source)
            .expect("unresolved import is not a parsing error")
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

fn change(state: &mut Workspace, url: &Uri<String>, text: &str, version: i32) {
    state
        .apply_change(lsp_types::DidChangeTextDocumentParams {
            text_document: lsp_types::VersionedTextDocumentIdentifier {
                uri: url.as_str().parse().unwrap(),
                version,
            },
            content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.into(),
            }],
        })
        .unwrap();
}
