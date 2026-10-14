use fluent_uri::Uri;
use pkl_db::{
    Db, FileStatus, ImportResolution, IoUpdate, PendingIo, RootDatabase, resolve_module_uri,
};
use pkl_vfs::{FileError, Source, SourceLoader};
use std::{path::Path, sync::Arc};
struct NoIo;
impl SourceLoader for NoIo {
    fn load(&self, _: &Uri<String>, _: &Path) -> Result<Arc<Source>, FileError> {
        panic!("query read source")
    }
    fn list_files(&self, _: &Uri<String>) -> Result<Vec<Uri<String>>, FileError> {
        panic!("query read directory")
    }
}
#[test]
fn remote_import_requests_inputs_then_resolves_without_query_io() {
    let mut db = RootDatabase::new(Arc::new(NoIo), Default::default());
    let uri = Uri::parse("file:///main.pkl".to_owned()).unwrap();
    let remote = Uri::parse("https://example.test/lib.pkl".to_owned()).unwrap();
    let file = db
        .open_file(
            &uri,
            Arc::from("import \"https://example.test/lib.pkl\""),
            None,
        )
        .unwrap();
    assert!(matches!(
        resolve_module_uri(&db, file, remote.to_string(), false, None),
        ImportResolution::Unresolved { .. }
    ));
    assert!(
        db.pending_io()
            .iter()
            .any(|job| matches!(job, PendingIo::Source { uri, .. } if uri == &remote))
    );
    db.apply_source(&remote, Some(Arc::from("value = 42")))
        .unwrap();
    let target = db.file(&remote).unwrap();
    assert_eq!(
        resolve_module_uri(&db, file, remote.to_string(), false, None),
        ImportResolution::Single(target)
    );
    assert_eq!(target.status(&db), FileStatus::Present);
    assert_eq!(&**target.text(&db), "value = 42");
}
#[test]
fn cache_dir_change_requeues_failed_remote_attempt() {
    let mut db = RootDatabase::new(Arc::new(NoIo), Default::default());
    let uri = Uri::parse("https://example.test/lib.pkl".to_owned()).unwrap();
    db.file(&uri).unwrap();
    db.apply_source(&uri, None).unwrap();
    assert!(db.pending_io().is_empty());
    let mut settings = pkl_db::settings::WorkspaceSettings {
        cache_dir: Some("/different-cache".into()),
        ..Default::default()
    };
    settings.resolve_paths(&[]);
    db.set_settings(settings);
    assert!(
        db.pending_io().iter().any(
            |job| matches!(job, PendingIo::Source { uri: requested, .. } if requested == &uri)
        )
    );
}
#[test]
fn modulepath_backing_change_is_applied_as_an_input() {
    let settings = pkl_db::settings::WorkspaceSettings {
        module_paths: vec!["/modules".into()],
        ..Default::default()
    };
    let mut db = RootDatabase::new(Arc::new(NoIo), settings);
    let file = db
        .open_file(
            &Uri::parse("file:///main.pkl".to_owned()).unwrap(),
            Arc::from("import \"modulepath:/target.pkl\""),
            None,
        )
        .unwrap();
    let reference = "modulepath:/target.pkl".to_owned();
    assert!(matches!(
        resolve_module_uri(&db, file, reference.clone(), false, None),
        ImportResolution::Unresolved { .. }
    ));
    let request = db
        .pending_io()
        .into_iter()
        .find_map(|job| {
            if let PendingIo::Routing(request) = job {
                Some(request)
            } else {
                None
            }
        })
        .unwrap();
    db.apply_io(IoUpdate::Routing {
        request: request.clone(),
        root: Some(Uri::parse("file:///modules/".to_owned()).unwrap()),
    })
    .unwrap();
    let target_uri = Uri::parse("file:///modules/target.pkl".to_owned()).unwrap();
    let target = db
        .apply_source(&target_uri, Some(Arc::from("value = 1")))
        .unwrap();
    assert_eq!(
        resolve_module_uri(&db, file, reference.clone(), false, None),
        ImportResolution::Single(target)
    );
    db.request_routing_refresh();
    db.apply_io(IoUpdate::Routing {
        request,
        root: Some(Uri::parse("jar:file:///modules!/".to_owned()).unwrap()),
    })
    .unwrap();
    assert!(matches!(
        resolve_module_uri(&db, file, reference, false, None),
        ImportResolution::Unresolved { .. }
    ));
}
