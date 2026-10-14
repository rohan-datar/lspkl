use fluent_uri::Uri;
use parking_lot::Mutex;
use pkl_db::{Db, File, FileStatus, RootDatabase};
use pkl_diagnostics::{Diagnostic, QuickFix, Severity, diagnostics};
use pkl_vfs::StandardSourceLoader;
use std::sync::Arc;

fn database() -> (RootDatabase, Arc<Mutex<Vec<String>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let log = events.clone();
    let db = RootDatabase::with_event_callback(
        Arc::new(StandardSourceLoader::default()),
        Default::default(),
        Some(Box::new(move |event| {
            if let salsa::EventKind::WillExecute { database_key } = event.kind {
                log.lock().push(format!("{database_key:?}"));
            }
        })),
    );
    (db, events)
}

fn module(db: &mut RootDatabase, name: &str, text: &str) -> File {
    db.upsert_file(
        &Uri::parse(format!("file:///diagnostic-tests/{name}.pkl")).unwrap(),
        Arc::from(text),
        None,
        FileStatus::Present,
    )
    .unwrap()
}

fn executions(events: &Mutex<Vec<String>>, query: &str) -> usize {
    let prefix = format!("{query}(");
    events
        .lock()
        .iter()
        .filter(|event| event.starts_with(&prefix))
        .count()
}

// A tracked downstream consumer models code-action extraction from the same
// production result, and makes diagnostic equality/backdating observable.
#[salsa::tracked(returns(clone))]
fn code_action_fixes(db: &dyn Db, file: File) -> Arc<[QuickFix]> {
    diagnostics(db, file)
        .iter()
        .flat_map(|diagnostic| diagnostic.quick_fixes.iter().cloned())
        .collect::<Vec<_>>()
        .into()
}

fn local_diagnostic(diagnostics: &[Diagnostic]) -> &Diagnostic {
    diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message == "missing modifier `local`")
        .unwrap()
}

#[test]
fn unchanged_queries_reuse_diagnostics_and_code_action_fixes() {
    let (mut db, events) = database();
    let file = module(&mut db, "root", "x = new Dynamic { value: Int = 1 }\n");
    let published = diagnostics(&db, file);
    let diagnostic = local_diagnostic(&published);
    assert_eq!(diagnostic.severity, Severity::Error);
    assert_eq!(diagnostic.quick_fixes[0].title, "add modifier `local`");
    assert_eq!(diagnostic.quick_fixes[0].edits[0].new_text, "local ");
    events.lock().clear();
    let fixes = code_action_fixes(&db, file);
    assert_eq!(&fixes[0], &diagnostic.quick_fixes[0]);
    assert_eq!(executions(&events, "diagnostics"), 0);
    events.lock().clear();
    assert!(Arc::ptr_eq(&published, &diagnostics(&db, file)));
    assert!(Arc::ptr_eq(&fixes, &code_action_fixes(&db, file)));
    assert!(events.lock().is_empty());

    module(&mut db, "unrelated", "unrelated = 42");
    events.lock().clear();
    assert_eq!(published, diagnostics(&db, file));
    assert_eq!(fixes, code_action_fixes(&db, file));
    assert_eq!(executions(&events, "diagnostics"), 0);
    assert_eq!(executions(&events, "code_action_fixes"), 0);
}

#[test]
fn source_edits_refresh_ranges_and_fixes_and_equal_results_backdate() {
    let (mut db, events) = database();
    let source = "x = new Dynamic { value: Int = 1 }\n";
    let file = module(&mut db, "root", source);
    let initial = diagnostics(&db, file);
    let initial_diagnostic = local_diagnostic(&initial);
    let initial_fixes = code_action_fixes(&db, file);
    let prefix = "// leading comment\n";
    let edited = format!("{prefix}{source}");
    module(&mut db, "root", &edited);
    events.lock().clear();
    let refreshed = diagnostics(&db, file);
    let refreshed_diagnostic = local_diagnostic(&refreshed);
    assert_eq!(refreshed_diagnostic.message, initial_diagnostic.message);
    assert_eq!(
        u32::from(refreshed_diagnostic.span.start()),
        u32::from(initial_diagnostic.span.start()) + prefix.len() as u32
    );
    assert_eq!(
        u32::from(refreshed_diagnostic.span.end()),
        u32::from(initial_diagnostic.span.end()) + prefix.len() as u32
    );
    let refreshed_fixes = code_action_fixes(&db, file);
    assert_ne!(initial_fixes, refreshed_fixes);
    let edit = &refreshed_fixes[0].edits[0];
    assert_eq!(
        u32::from(edit.span.start()),
        u32::from(initial_fixes[0].edits[0].span.start()) + prefix.len() as u32
    );
    assert!(edit.span.is_empty());
    assert_eq!(edit.new_text, "local ");
    assert_eq!(executions(&events, "diagnostics"), 1);
    assert_eq!(executions(&events, "code_action_fixes"), 1);

    // A source-only trailing comment changes parsing, but not any diagnostic.
    module(&mut db, "root", &format!("{edited}// trailing comment\n"));
    events.lock().clear();
    assert_eq!(refreshed_fixes, code_action_fixes(&db, file));
    assert_eq!(refreshed, diagnostics(&db, file));
    assert_eq!(executions(&events, "diagnostics"), 1);
    assert_eq!(executions(&events, "code_action_fixes"), 0);

    let mut fixed = edited;
    fixed.insert_str(usize::from(edit.span.start()), &edit.new_text);
    module(&mut db, "root", &fixed);
    events.lock().clear();
    assert!(diagnostics(&db, file).is_empty());
    assert!(code_action_fixes(&db, file).is_empty());
    assert_eq!(executions(&events, "diagnostics"), 1);
    assert_eq!(executions(&events, "code_action_fixes"), 1);
}

#[test]
fn imported_semantic_changes_invalidate_root_diagnostics() {
    let (mut db, events) = database();
    module(&mut db, "target", "value: Int = 1\n");
    let source = "import \"target.pkl\"\nvalue: Int = target.value\n";
    let file = module(&mut db, "root", source);
    assert!(diagnostics(&db, file).is_empty());
    assert!(code_action_fixes(&db, file).is_empty());
    module(&mut db, "target", "value: String = \"changed\"\n");
    events.lock().clear();
    let refreshed = diagnostics(&db, file);
    let mismatch = refreshed
        .iter()
        .find(|diagnostic| diagnostic.message == "Type mismatch. Expected: Int. Actual: String")
        .unwrap();
    assert_eq!(mismatch.severity, Severity::Error);
    assert_eq!(
        &source[usize::from(mismatch.span.start())..usize::from(mismatch.span.end())],
        "target.value"
    );
    assert_eq!(executions(&events, "diagnostics"), 1);
    assert_eq!(
        executions(&events, "parse"),
        1,
        "only the imported source should reparse"
    );
    events.lock().clear();
    assert!(code_action_fixes(&db, file).is_empty());
    assert_eq!(executions(&events, "diagnostics"), 0);
    assert_eq!(executions(&events, "code_action_fixes"), 1);

    module(&mut db, "target", "value: Int = 2\n");
    events.lock().clear();
    assert!(diagnostics(&db, file).is_empty());
    assert_eq!(executions(&events, "diagnostics"), 1);
}
