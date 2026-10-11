//! Stable file identity contracts across distinct URIs and editor lifecycles.

use std::sync::Arc;

use fluent_uri::Uri;
use lspkl::ServerState;
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

    assert_ne!(first_id, FileId::BASE);
    assert_ne!(second_id, FileId::BASE);
    assert_ne!(first_id, second_id);
    assert_eq!(registry.uri_for(first_id), Some(&first));
    assert_eq!(registry.uri_for(second_id), Some(&second));
    assert_eq!(registry.register(&uri(first.as_str())), first_id);
}

#[test]
fn editor_changes_do_not_change_the_file_identity() {
    let state = ServerState::new();
    let url = uri("file:///workspace/library.pkl");
    let file = state
        .file_manager
        .open(url.clone(), "oldValue = 1\n".into(), Some(1));
    let before = state.file_registry.lock().register(&file.url);

    let updated = state
        .file_manager
        .update(&url, "newValue = 2\n".into(), Some(2))
        .unwrap();
    let mut registry = state.file_registry.lock();
    let after = registry.register(&updated.url);

    assert_eq!(after, before);
    assert_eq!(registry.uri_for(after), Some(&url));
}

#[test]
fn closing_and_reopening_preserves_identity_without_reusing_it_for_another_uri() {
    let state = ServerState::new();
    let url = uri("file:///workspace/library.pkl");
    let other_url = uri("file:///workspace/other.pkl");
    let original = state
        .file_manager
        .open(url.clone(), "value = 1\n".into(), Some(1));
    let original_id = state.file_registry.lock().register(&original.url);
    state.file_manager.close(&url);

    assert!(state.file_manager.get(&url).is_none());
    {
        let mut registry = state.file_registry.lock();
        assert_eq!(registry.uri_for(original_id), Some(&url));
        let other_id = registry.register(&other_url);
        assert_ne!(other_id, original_id);
        assert_eq!(registry.uri_for(other_id), Some(&other_url));
    }

    let reopened = state
        .file_manager
        .open(url.clone(), "value = 2\n".into(), Some(2));
    assert!(!Arc::ptr_eq(&original, &reopened));
    let mut registry = state.file_registry.lock();
    assert_eq!(registry.register(&reopened.url), original_id);
    assert_eq!(registry.uri_for(original_id), Some(&url));
}

#[test]
fn base_and_unknown_ids_have_no_user_uri() {
    let mut registry = FileRegistry::new();
    assert!(registry.uri_for(FileId::BASE).is_none());
    assert!(registry.uri_for(FileId(u32::MAX)).is_none());

    let id = registry.register(&uri("file:///workspace/main.pkl"));
    assert_ne!(id, FileId::BASE);
    assert!(registry.uri_for(FileId::BASE).is_none());
    assert!(registry.uri_for(FileId(u32::MAX)).is_none());
}
