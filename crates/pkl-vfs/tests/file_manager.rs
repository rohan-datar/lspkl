//! Integration tests for the file manager.
//!
//! These tests exercise the public API of `FileManager` and `VirtualFile`.
use std::sync::Arc;

use fluent_uri::Uri;
use pkl_vfs::{FileManager, Origin, VirtualFile};

fn ephemeral_url(name: &str) -> Uri<String> {
    // Ephemeral / virtual-only URIs use a synthetic scheme so they don't
    // collide with real `file://` paths.
    Uri::parse(format!("inmemory:///{name}")).unwrap()
}

#[test]
fn new_manager_is_empty() {
    let mgr = FileManager::new();
    assert!(mgr.is_empty());
    assert_eq!(mgr.len(), 0);
}

#[test]
fn open_then_get_returns_same_file() {
    let mgr = FileManager::new();
    let url = ephemeral_url("a.pkl");

    let opened = mgr.open(url.clone(), "foo = 1".into(), Some(1));
    let fetched = mgr.get(&url).expect("file should be tracked after open");

    assert!(
        Arc::ptr_eq(&opened, &fetched),
        "get must return the same Arc as open"
    );
    assert_eq!(opened.contents(), "foo = 1");
    assert_eq!(opened.version(), Some(1));
}

#[test]
fn get_returns_none_for_unknown_url() {
    let mgr = FileManager::new();
    let url = ephemeral_url("unknown.pkl");
    assert!(mgr.get(&url).is_none());
}

#[test]
fn update_replaces_contents_and_invalidates_cache() {
    let mgr = FileManager::new();
    let url = ephemeral_url("b.pkl");

    let file = mgr.open(url.clone(), "foo = 1".into(), Some(1));
    // Force a parse to populate the cache.
    let _ = mgr.parse(&url).expect("first parse should succeed");
    assert!(
        file.cached_tree().is_some(),
        "parse must populate the cache"
    );

    mgr.update(&url, "foo = 2".into(), Some(2)).unwrap();

    assert_eq!(file.contents(), "foo = 2");
    assert_eq!(file.version(), Some(2));
    assert!(
        file.cached_tree().is_none(),
        "updating contents must invalidate the cached tree"
    );
}

#[test]
fn update_unknown_url_returns_err() {
    let mgr = FileManager::new();
    let url = ephemeral_url("never-opened.pkl");
    let result = mgr.update(&url, "x = 1".into(), Some(1));
    assert!(result.is_err(), "updating an unopened file must error");
}

#[test]
fn close_removes_file_from_registry() {
    let mgr = FileManager::new();
    let url = ephemeral_url("c.pkl");

    mgr.open(url.clone(), "x = 1".into(), Some(1));
    assert_eq!(mgr.len(), 1);

    mgr.close(&url);
    assert!(mgr.get(&url).is_none());
    assert_eq!(mgr.len(), 0);
}

#[test]
fn close_is_noop_for_unknown_url() {
    let mgr = FileManager::new();
    let url = ephemeral_url("never-opened.pkl");
    // Must not panic.
    mgr.close(&url);
}

#[test]
fn parse_returns_a_tree() {
    let mgr = FileManager::new();
    let url = ephemeral_url("d.pkl");
    mgr.open(url.clone(), "foo = 1".into(), Some(1));

    let tree = mgr.parse(&url).expect("parse should succeed for valid Pkl");
    assert!(tree.node_count() > 0);
}

#[test]
fn parse_caches_the_tree() {
    let mgr = FileManager::new();
    let url = ephemeral_url("e.pkl");
    mgr.open(url.clone(), "foo = 1".into(), Some(1));

    let first = mgr.parse(&url).unwrap();
    let second = mgr.parse(&url).unwrap();

    assert!(
        Arc::ptr_eq(&first, &second),
        "parse must return the cached Arc on the second call"
    );
}

#[test]
fn parse_after_update_reparses() {
    let mgr = FileManager::new();
    let url = ephemeral_url("f.pkl");
    mgr.open(url.clone(), "foo = 1".into(), Some(1));

    let first = mgr.parse(&url).unwrap();
    mgr.update(&url, "foo = 2".into(), Some(2)).unwrap();
    let second = mgr.parse(&url).unwrap();

    assert!(
        !Arc::ptr_eq(&first, &second),
        "parse must produce a fresh tree after the contents change"
    );
}

#[test]
fn ephemeral_constructor_round_trips() {
    let url = ephemeral_url("g.pkl");
    let file = VirtualFile::new(url.clone(), Origin::Ephemeral, "x = 1".into(), Some(1));

    assert_eq!(file.url, url);
    assert_eq!(file.origin, Origin::Ephemeral);
    assert_eq!(file.contents(), "x = 1");
    assert_eq!(file.version(), Some(1));
    assert!(file.cached_tree().is_none());
}

#[test]
fn url_to_path_rejects_non_file_scheme() {
    use pkl_vfs::file_manager::url_to_path;
    let url = Uri::parse(String::from("https://example.com/foo.pkl")).unwrap();
    assert!(url_to_path(&url).is_err());
}

#[test]
fn url_to_path_accepts_file_scheme() {
    use pkl_vfs::file_manager::url_to_path;
    let workspace = tempfile::tempdir().unwrap();
    let expected = workspace.path().join("space and λ.pkl");
    let url = pkl_vfs::path_to_uri(&expected).unwrap();
    let path = url_to_path(&url).expect("file:// URLs must convert to a path");
    assert_eq!(path, expected);
}

#[test]
fn get_or_load_reads_from_disk() {
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut tmp = NamedTempFile::new().unwrap();
    writeln!(tmp, "foo = 1").unwrap();
    let url = pkl_vfs::path_to_uri(tmp.path()).expect("tempfile path must convert");

    let mgr = FileManager::new();
    let file = mgr
        .get_or_load(&url)
        .expect("get_or_load should read existing files");

    assert_eq!(file.contents().trim_end(), "foo = 1");
    assert!(matches!(file.origin, Origin::Fs { .. }));
}

#[test]
fn get_or_load_rejects_unsupported_scheme() {
    let mgr = FileManager::new();
    let url = Uri::parse(String::from("http://example.com/foo.pkl")).unwrap();
    let result = mgr.get_or_load(&url);
    assert!(matches!(
        result,
        Err(pkl_vfs::FileError::UnsupportedScheme(_))
    ));
}

#[test]
fn concurrent_get_is_safe() {
    // FileManager is supposed to be Send+Sync. Spin up several threads, all
    // touching the same URLs; if it deadlocks or panics, the test fails.
    use std::thread;

    let mgr = Arc::new(FileManager::new());
    for i in 0..16 {
        mgr.open(
            ephemeral_url(&format!("c{i}.pkl")),
            format!("x = {i}"),
            Some(1),
        );
    }

    let mut handles = vec![];
    for _ in 0..8 {
        let mgr = Arc::clone(&mgr);
        handles.push(thread::spawn(move || {
            for i in 0..16 {
                let url = ephemeral_url(&format!("c{i}.pkl"));
                let file = mgr.get(&url).expect("file should be present");
                assert!(!file.contents().is_empty());
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}
