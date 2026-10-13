use fluent_uri::Uri;
use parking_lot::Mutex;
use pkl_db::{FileStatus, RootDatabase};
use pkl_types::{DeclId, InFile, ItemId, Type, decl_type, module_items};
use pkl_vfs::StandardSourceLoader;
use std::sync::Arc;

#[test]
fn hover_reuses_the_body_inferred_by_diagnostics() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let log = events.clone();
    let mut db = RootDatabase::with_event_callback(
        Arc::new(StandardSourceLoader::default()),
        Default::default(),
        Some(Box::new(move |event| {
            if let salsa::EventKind::WillExecute { database_key } = event.kind {
                log.lock().push(format!("{database_key:?}"));
            }
        })),
    );
    let source = "probe = 42 is Int\nresult = probe\n";
    let file = db
        .upsert_file(
            &Uri::parse("file:///inference_reuse.pkl".to_owned()).unwrap(),
            Arc::from(source),
            None,
            FileStatus::Present,
        )
        .unwrap();
    let workspace = lspkl::Workspace::with_database(db);
    let snapshot = workspace.snapshot();
    let ctx = snapshot.context(file);
    let diagnostics = snapshot.diagnostics().analyze(&ctx, file);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        events
            .lock()
            .iter()
            .filter(|event| event.contains("infer_body"))
            .count(),
        1
    );
    events.lock().clear();
    let hover = lsp_features::hover::hover(
        &ctx,
        InFile::new(file, (source.rfind("probe").unwrap() as u32).into()),
    )
    .unwrap();
    let lsp_types::HoverContents::Markup(content) = hover.contents else {
        panic!("expected typed hover");
    };
    assert!(
        content.value.contains("probe: Boolean"),
        "{}",
        content.value
    );
    assert!(
        !events
            .lock()
            .iter()
            .any(|event| event.contains("infer_body") || event.contains("expression_query")),
        "{:?}",
        events.lock()
    );
}

#[test]
fn explicit_declaration_types_do_not_depend_on_initializer_positions() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let log = events.clone();
    let mut db = RootDatabase::with_event_callback(
        Arc::new(StandardSourceLoader::default()),
        Default::default(),
        Some(Box::new(move |event| {
            if let salsa::EventKind::WillExecute { database_key } = event.kind {
                log.lock().push(format!("{database_key:?}"));
            }
        })),
    );
    let uri = Uri::parse("file:///annotation_firewall.pkl".to_owned()).unwrap();
    let file = db
        .upsert_file(
            &uri,
            Arc::from("prefix = 1\nvalue: String = \"before\"\n"),
            None,
            FileStatus::Present,
        )
        .unwrap();
    let item = module_items(&db, file)
        .properties
        .iter()
        .find(|(_, property)| property.declaration.name.text(&db) == "value")
        .unwrap()
        .0;
    let declaration = DeclId {
        file,
        root: file,
        item: ItemId::Property(item),
    };
    let before = decl_type(&db, declaration);
    assert_eq!(before, *pkl_types::base_module(&db).string_type());
    events.lock().clear();
    db.upsert_file(
        &uri,
        Arc::from("prefix = if (true) (1 + 2) else 3\nvalue: String = \"after\"\n"),
        None,
        FileStatus::Present,
    )
    .unwrap();
    assert_eq!(decl_type(&db, declaration), before);
    assert!(!matches!(before, Type::Unknown));
    assert!(
        !events
            .lock()
            .iter()
            .any(|event| event.contains("declaration_type")
                || event.contains("resolve_request")
                || event.contains("infer_body")),
        "{:?}",
        events.lock()
    );
}
