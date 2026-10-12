//! Stable file identity contracts across distinct URIs and editor lifecycles.

use std::sync::Arc;

use fluent_uri::Uri;
use lspkl::Workspace;
use lspkl::file_registry::FileRegistry;
use pkl_types::FileId;

fn uri(text: &str) -> Uri<String> {
    Uri::parse(text.to_owned()).unwrap()
}

#[test]
fn distinct_uris_with_the_same_basename_have_distinct_non_base_ids() {
    let first = uri("file:///workspace/first/library.pkl");
    let second = uri("file:///workspace/second/library.pkl");
    let mut registry = FileRegistry::new();

    let first_id = registry.register(&first);
    let second_id = registry.register(&second);

    let base = registry.register(&uri("pkl:base"));
    assert_ne!(first_id, base);
    assert_ne!(second_id, base);
    assert_ne!(first_id, second_id);
    assert_eq!(registry.uri_for(first_id), Some(&first));
    assert_eq!(registry.uri_for(second_id), Some(&second));
    assert_eq!(registry.register(&uri(first.as_str())), first_id);
}

#[test]
fn editor_changes_do_not_change_the_file_identity() {
    let mut state = Workspace::new();
    let url = uri("file:///workspace/library.pkl");
    let file = open(&mut state, url.clone(), "oldValue = 1\n".into(), Some(1));
    let before = state.snapshot().file_registry.lock().register(&file.url);

    change(&mut state, &url, "newValue = 2\n", 2);
    let updated = state.snapshot().document(url.as_str()).unwrap();
    let view = state.snapshot();
    let mut registry = view.file_registry.lock();
    let after = registry.register(&updated.url);

    assert_eq!(after, before);
    assert_eq!(registry.uri_for(after), Some(&url));
}

#[test]
fn closing_and_reopening_preserves_identity_without_reusing_it_for_another_uri() {
    let mut state = Workspace::new();
    let url = uri("file:///workspace/library.pkl");
    let other_url = uri("file:///workspace/other.pkl");
    let original = open(&mut state, url.clone(), "value = 1\n".into(), Some(1));
    let original_id = state
        .snapshot()
        .file_registry
        .lock()
        .register(&original.url);
    state
        .apply_close(lsp_types::DidCloseTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier {
                uri: url.as_str().parse().unwrap(),
            },
        })
        .unwrap();

    assert!(state.snapshot().document(url.as_str()).is_none());
    {
        let view = state.snapshot();
        let mut registry = view.file_registry.lock();
        assert_eq!(registry.uri_for(original_id), Some(&url));
        let other_id = registry.register(&other_url);
        assert_ne!(other_id, original_id);
        assert_eq!(registry.uri_for(other_id), Some(&other_url));
    }

    let reopened = open(&mut state, url.clone(), "value = 2\n".into(), Some(2));
    assert!(!Arc::ptr_eq(&original, &reopened));
    let view = state.snapshot();
    let mut registry = view.file_registry.lock();
    assert_eq!(registry.register(&reopened.url), original_id);
    assert_eq!(registry.uri_for(original_id), Some(&url));
}

#[test]
fn base_is_registered_and_addressable_like_every_other_file() {
    let mut registry = FileRegistry::new();
    let base = uri("pkl:base");
    let base_id = registry.register(&base);
    assert_eq!(registry.uri_for(base_id), Some(&base));
    assert!(registry.uri_for(FileId(u32::MAX)).is_none());

    let main = uri("file:///workspace/main.pkl");
    let main_id = registry.register(&main);
    assert_ne!(main_id, base_id);
    assert_eq!(registry.uri_for(main_id), Some(&main));
    assert_eq!(registry.register(&base), base_id);
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
