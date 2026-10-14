//! Imported-module parsing and owned tree observations across editor updates.

mod support;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use fluent_uri::Uri;
use lspkl::Workspace;
use pkl_db::{File, ImportResolution, parse, resolve_import};
use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};

fn file_uri(path: &Path) -> Uri<String> {
    Uri::parse(url::Url::from_file_path(path).unwrap().to_string()).unwrap()
}

fn importing_source(state: &mut Workspace, dir: &Path) -> (File, Arc<SyntaxTree>, NodeId) {
    let file = open(
        state,
        file_uri(&dir.join("main.pkl")),
        "import \"./library.pkl\"\n".into(),
        Some(1),
    );
    let tree = parse(state.database(), file).as_ref().unwrap().clone();
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
    let (source, _tree, import) = importing_source(&mut state, temp.path());

    let ImportResolution::Single(target) = resolve_import(state.database(), source, import, None)
    else {
        panic!("target must resolve")
    };
    let target_tree = parse(state.database(), target).as_ref().unwrap();

    assert_eq!(*target.uri(state.database()), file_uri(&target_path));
    assert_eq!(property_name(target_tree), "answer");
    assert_eq!(target_tree.source(), "answer = 42\n");
}

#[test]
fn editor_changes_refresh_imported_tree_without_mutating_owned_observations() {
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
    let (source, _tree, import) = importing_source(&mut state, temp.path());
    let ImportResolution::Single(before_file) =
        resolve_import(state.database(), source, import, None)
    else {
        panic!("target must resolve")
    };
    let before = parse(state.database(), before_file)
        .as_ref()
        .unwrap()
        .clone();
    assert_eq!(before_file, opened);
    assert_eq!(property_name(&before), "oldValue");

    change(&mut state, &file_uri(&target_path), "newValue = 2\n", 2);
    let ImportResolution::Single(after_file) =
        resolve_import(state.database(), source, import, None)
    else {
        panic!("target must resolve")
    };
    let after = parse(state.database(), after_file).as_ref().unwrap();

    assert_eq!(after_file.version(state.database()), Some(2));
    assert_eq!(property_name(after), "newValue");
    assert_eq!(after.source(), "newValue = 2\n");
    assert_eq!(property_name(&before), "oldValue");
    assert_eq!(before.source(), "oldValue = 1\n");
}

#[test]
fn missing_target_is_unresolved_rather_than_a_parse_error() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = Workspace::new();
    let (source, _tree, import) = importing_source(&mut state, temp.path());

    assert!(parse(state.database(), source).is_ok());
    assert!(matches!(
        resolve_import(state.database(), source, import, None),
        ImportResolution::Unresolved {
            package_to_download: None
        }
    ));
}

fn open(
    state: &mut Workspace,
    url: Uri<String>,
    text: String,
    version: Option<i32>,
) -> pkl_db::File {
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
    support::preload(state);
    state.document(url.as_str()).unwrap()
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
    support::preload(state);
}
