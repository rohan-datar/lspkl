//! Immutable SDK bootstrap must survive cancellation of the first document analysis.
use lspkl::{Workspace, handlers::HandlerError};
use pkl_db::{Db, RootDatabase};
use pkl_types::NodeRef;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[test]
fn first_analysis_cancellation_does_not_restart_sdk_parsing() {
    let armed = Arc::new(AtomicBool::new(false));
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let executions = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let callback_armed = armed.clone();
    let callback_release = release.clone();
    let callback_executions = executions.clone();
    let db = RootDatabase::with_event_callback(
        Arc::new(pkl_vfs::StandardSourceLoader::default()),
        Default::default(),
        Some(Box::new(move |event| match event.kind {
            salsa::EventKind::WillExecute { database_key } => {
                callback_executions.lock().push(format!("{database_key:?}"));
                if callback_armed.swap(false, Ordering::SeqCst) {
                    entered_tx.send(()).unwrap();
                    let (lock, condition) = &*callback_release;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = condition.wait(released).unwrap();
                    }
                }
            }
            salsa::EventKind::DidSetCancellationFlag => {
                let (lock, condition) = &*callback_release;
                *lock.lock().unwrap() = true;
                condition.notify_all();
            }
            _ => {}
        })),
    );
    let base = db
        .file(&fluent_uri::Uri::parse("pkl:base".to_owned()).unwrap())
        .unwrap();
    let sdk_parse_key = format!("parse({:?})", salsa::plumbing::AsId::as_id(&base));
    let mut workspace = Workspace::with_database(db);
    let uri: lsp_types::Uri = "file:///sdk-bootstrap.pkl".parse().unwrap();
    workspace
        .apply_open(lsp_types::DidOpenTextDocumentParams {
            text_document: lsp_types::TextDocumentItem {
                uri: uri.clone(),
                language_id: "pkl".into(),
                version: 1,
                text: "value: Int = 1".into(),
            },
        })
        .unwrap();
    let file = workspace.document(uri.as_str()).unwrap();
    *release.0.lock().unwrap() = false;
    armed.store(true, Ordering::SeqCst);
    let snapshot = workspace.snapshot();
    let reader = std::thread::spawn(move || {
        snapshot.run("sdk_bootstrap", |snapshot| {
            let ctx = snapshot.context(file);
            Ok(ctx.tree(file).unwrap().source().to_owned())
        })
    });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("analysis must reach a query");
    workspace
        .apply_change(lsp_types::DidChangeTextDocumentParams {
            text_document: lsp_types::VersionedTextDocumentIdentifier { uri, version: 2 },
            content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: r#"value: String = "new""#.into(),
            }],
        })
        .unwrap();
    assert!(matches!(
        reader.join().unwrap(),
        Err(HandlerError::ContentModified)
    ));
    let snapshot = workspace.snapshot();
    let ctx = snapshot.context(file);
    let tree = ctx.tree(file).unwrap();
    let property = tree
        .child(tree.root(), |kind| {
            matches!(kind, pkl_syntax::ast::NodeKind::ClassProperty)
        })
        .unwrap();
    assert_eq!(
        pkl_types::resolver::definition_type(NodeRef::new(file, property), &ctx).render(&ctx),
        "String"
    );
    assert_eq!(
        executions
            .lock()
            .iter()
            .filter(|key| **key == sdk_parse_key)
            .count(),
        1,
        "cancelling first document analysis must not restart immutable SDK parsing"
    );
}
