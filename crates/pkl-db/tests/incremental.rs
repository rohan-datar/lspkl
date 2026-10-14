use fluent_uri::Uri;
use parking_lot::Mutex;
use pkl_db::{Db, File, FileStatus, ImportResolution, RootDatabase, parse, resolve_import};
use pkl_syntax::ast::{NodeId, NodeKind};
use pkl_vfs::{StandardSourceLoader, path_to_uri};
use salsa::Setter;
use std::sync::Arc;

fn load_source(db: &mut RootDatabase, uri: &Uri<String>) -> Result<File, pkl_vfs::FileError> {
    let loader = db.source_loader_handle();
    let text = loader
        .load(uri, db.config().cache_dir(db))
        .ok()
        .map(|source| source.text.clone());
    db.apply_source(uri, text)
}
fn close_editor(db: &mut RootDatabase, uri: &Uri<String>) -> Result<File, pkl_vfs::FileError> {
    if let Some(file) = db.known_file(uri) {
        file.set_status(db).to(FileStatus::Present);
        file.set_version(db).to(None);
    }
    load_source(db, uri)
}
fn refresh_inputs(db: &mut RootDatabase) {
    db.request_routing_refresh();
    let loader = db.source_loader_handle();
    let updates = db
        .directory_roots()
        .iter()
        .map(|root| pkl_db::DirectoryUpdate::load(&*loader, root))
        .collect();
    db.apply_directory_updates(updates);
    for file in db.all_files() {
        let _ = pkl_db::import_graph(db, file);
    }
    settle_inputs(db);
}
fn settle_inputs(db: &mut RootDatabase) {
    let loader = db.source_loader_handle();
    loop {
        let jobs = db.pending_io();
        if jobs.is_empty() {
            break;
        }
        for job in jobs {
            let update = match job {
                pkl_db::PendingIo::Source { uri, cache_dir } => pkl_db::IoUpdate::Source {
                    text: loader
                        .load(&uri, &cache_dir)
                        .ok()
                        .map(|source| source.text.clone()),
                    uri,
                },
                job => job.load(&*loader, &StandardSourceLoader::new()),
            };
            db.apply_io(update).unwrap();
        }
        for file in db.all_files() {
            let _ = pkl_db::import_graph(db, file);
        }
    }
}

fn import_owner(db: &RootDatabase, file: File) -> NodeId {
    let tree = parse(db, file).as_ref().unwrap();
    *tree
        .children(tree.root())
        .iter()
        .find(|&&node| matches!(tree.kind(node), NodeKind::Import | NodeKind::ImportGlob))
        .unwrap()
}
fn module(db: &mut RootDatabase, path: &std::path::Path, text: &str) -> File {
    let file = db
        .upsert_file(
            &path_to_uri(path).unwrap(),
            Arc::from(text),
            None,
            FileStatus::Present,
        )
        .unwrap();
    for source in db.all_files() {
        let _ = pkl_db::import_graph(db, source);
    }
    settle_inputs(db);
    file
}

fn edge(db: &RootDatabase, root: File, file: File, owner: NodeId) -> ImportResolution {
    pkl_db::import_graph(db, root).edges[&(file, owner)].clone()
}

#[test]
fn editing_a_executes_only_a_parse_and_keeps_unrelated_import_memoized() {
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
    let root = tempfile::tempdir().unwrap();
    let a = module(&mut db, &root.path().join("a.pkl"), "a = 1");
    let target = module(&mut db, &root.path().join("target.pkl"), "value = 1");
    let b = module(
        &mut db,
        &root.path().join("b.pkl"),
        "import \"target.pkl\"\nb = 1",
    );
    let owner = import_owner(&db, b);
    parse(&db, a).as_ref().unwrap();
    assert_eq!(
        resolve_import(&db, b, owner, None),
        ImportResolution::Single(target)
    );
    events.lock().clear();
    let uri = a.uri(&db).clone();
    db.upsert_file(&uri, Arc::from("a = 2"), None, FileStatus::Present)
        .unwrap();
    parse(&db, a).as_ref().unwrap();
    parse(&db, b).as_ref().unwrap();
    assert_eq!(
        resolve_import(&db, b, owner, None),
        ImportResolution::Single(target)
    );
    let executions = events.lock();
    assert_eq!(
        executions
            .iter()
            .filter(|event| event.starts_with("parse("))
            .count(),
        1,
        "{executions:?}"
    );
    assert!(
        !executions
            .iter()
            .any(|event| event.starts_with("resolve_import(")),
        "{executions:?}"
    );
}

#[test]
fn missing_target_creation_and_deletion_change_the_existing_import() {
    let root = tempfile::tempdir().unwrap();
    let mut db = RootDatabase::default();
    let file = module(
        &mut db,
        &root.path().join("main.pkl"),
        "import \"target.pkl\"",
    );
    let owner = import_owner(&db, file);
    assert!(matches!(
        resolve_import(&db, file, owner, None),
        ImportResolution::Unresolved { .. }
    ));
    let target_uri = path_to_uri(&root.path().join("target.pkl")).unwrap();
    let target = db.file(&target_uri).unwrap();
    assert_eq!(target.status(&db), FileStatus::Missing);
    std::fs::write(root.path().join("target.pkl"), "value = 1").unwrap();
    load_source(&mut db, &target_uri).unwrap();
    assert_eq!(
        resolve_import(&db, file, owner, None),
        ImportResolution::Single(target)
    );
    std::fs::remove_file(root.path().join("target.pkl")).unwrap();
    load_source(&mut db, &target_uri).unwrap();
    assert!(matches!(
        resolve_import(&db, file, owner, None),
        ImportResolution::Unresolved { .. }
    ));
}

#[test]
fn watched_glob_membership_changes_without_editing_importer() {
    let root = tempfile::tempdir().unwrap();
    let mut db = RootDatabase::default();
    let file = module(
        &mut db,
        &root.path().join("main.pkl"),
        "import* \"targets/*.pkl\"",
    );
    let owner = import_owner(&db, file);
    assert_eq!(
        resolve_import(&db, file, owner, None),
        ImportResolution::Glob(Vec::new())
    );
    std::fs::create_dir(root.path().join("targets")).unwrap();
    let path = root.path().join("targets/a.pkl");
    std::fs::write(&path, "value = 1").unwrap();
    refresh_inputs(&mut db);
    let target = db.file(&path_to_uri(&path).unwrap()).unwrap();
    assert_eq!(
        resolve_import(&db, file, owner, None),
        ImportResolution::Glob(vec![target])
    );
    std::fs::remove_file(&path).unwrap();
    load_source(&mut db, &path_to_uri(&path).unwrap()).unwrap();
    refresh_inputs(&mut db);
    assert_eq!(
        resolve_import(&db, file, owner, None),
        ImportResolution::Glob(Vec::new())
    );
}

#[test]
fn backing_refresh_preserves_editor_text_until_close() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("main.pkl");
    let uri = path_to_uri(&path).unwrap();
    std::fs::write(&path, "value = 1").unwrap();
    let mut db = RootDatabase::default();
    let file = db.open_file(&uri, Arc::from("value = 2"), Some(3)).unwrap();
    std::fs::write(&path, "value = 4").unwrap();
    load_source(&mut db, &uri).unwrap();
    assert_eq!(&**file.text(&db), "value = 2");
    assert_eq!(file.version(&db), Some(3));
    close_editor(&mut db, &uri).unwrap();
    assert_eq!(&**file.text(&db), "value = 4");
    assert_eq!(file.version(&db), None);
    assert_eq!(file.status(&db), FileStatus::Present);
}

#[test]
fn sdk_editor_lifecycle_does_not_reparse_immutable_text() {
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
    let uri = Uri::parse("pkl:base".to_owned()).unwrap();
    let file = db.file(&uri).unwrap();
    let tree = parse(&db, file).as_ref().unwrap().clone();
    events.lock().clear();
    db.open_file(&uri, Arc::from("incorrect client contents"), Some(5))
        .unwrap();
    assert_eq!(file.status(&db), FileStatus::Open);
    assert_eq!(file.version(&db), Some(5));
    assert!(Arc::ptr_eq(&tree, parse(&db, file).as_ref().unwrap()));
    close_editor(&mut db, &uri).unwrap();
    assert_eq!(file.status(&db), FileStatus::Present);
    assert_eq!(file.version(&db), None);
    assert!(Arc::ptr_eq(&tree, parse(&db, file).as_ref().unwrap()));
    assert!(
        events
            .lock()
            .iter()
            .all(|event| !event.starts_with("parse("))
    );
}

fn project(root: &std::path::Path, modulepath: &str) -> Arc<pkl_project::PklProject> {
    Arc::new(pkl_project::PklProject {
        metadata: pkl_project::DerivedProjectMetadata {
            project_file_uri: path_to_uri(&root.join("PklProject")).unwrap().to_string(),
            package_uri: None,
            declared_dependencies: Default::default(),
            evaluator_settings: Some(pkl_project::EvaluatorSettings {
                module_cache_dir: None,
                module_path: Some(vec![modulepath.to_owned()]),
            }),
        },
        project_deps: None,
    })
}

#[test]
fn project_metadata_refresh_reroutes_only_its_own_imports_without_reparsing() {
    let root = tempfile::tempdir().unwrap();
    let a_root = root.path().join("a");
    let b_root = root.path().join("b");
    for path in [
        a_root.join("first"),
        a_root.join("second"),
        b_root.join("first"),
    ] {
        std::fs::create_dir_all(path).unwrap();
    }
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
    db.set_projects(vec![project(&a_root, "first"), project(&b_root, "first")]);
    let a = module(
        &mut db,
        &a_root.join("main.pkl"),
        r#"import "modulepath:/target.pkl""#,
    );
    let b = module(
        &mut db,
        &b_root.join("main.pkl"),
        r#"import "modulepath:/target.pkl""#,
    );
    let a_first = module(&mut db, &a_root.join("first/target.pkl"), "value = 1");
    let a_second = module(&mut db, &a_root.join("second/target.pkl"), "value = 2");
    let b_target = module(&mut db, &b_root.join("first/target.pkl"), "value = 3");
    let a_owner = import_owner(&db, a);
    let b_owner = import_owner(&db, b);
    assert_eq!(
        resolve_import(&db, a, a_owner, None),
        ImportResolution::Single(a_first)
    );
    assert_eq!(
        resolve_import(&db, b, b_owner, None),
        ImportResolution::Single(b_target)
    );
    events.lock().clear();
    db.set_projects(vec![project(&a_root, "second"), project(&b_root, "first")]);
    assert_eq!(
        resolve_import(&db, b, b_owner, None),
        ImportResolution::Single(b_target)
    );
    assert!(events.lock().is_empty());
    for file in db.all_files() {
        let _ = pkl_db::import_graph(&db, file);
    }
    settle_inputs(&mut db);
    assert_eq!(
        resolve_import(&db, a, a_owner, None),
        ImportResolution::Single(a_second)
    );
    assert!(
        events
            .lock()
            .iter()
            .all(|event| !event.starts_with("parse("))
    );
}

#[test]
fn remote_descendants_inherit_project_context_and_observe_its_refresh() {
    use pkl_vfs::{FileError, Source, SourceLoader};
    struct Loader;
    impl SourceLoader for Loader {
        fn load(&self, uri: &Uri<String>, _: &std::path::Path) -> Result<Arc<Source>, FileError> {
            let text = match uri.as_str() {
                "https://example.test/first.pkl" => "import \"second.pkl\"",
                "https://example.test/second.pkl" => "import \"modulepath:/target.pkl\"",
                _ => return Err(FileError::InvalidSource(uri.to_string())),
            };
            Ok(Arc::new(Source {
                url: uri.clone(),
                text: Arc::from(text),
            }))
        }
        fn list_files(&self, _: &Uri<String>) -> Result<Vec<Uri<String>>, FileError> {
            Ok(Vec::new())
        }
    }
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("first")).unwrap();
    std::fs::create_dir(root.path().join("second")).unwrap();
    let mut db = RootDatabase::new(Arc::new(Loader), Default::default());
    db.set_projects(vec![project(root.path(), "first")]);
    let local = module(
        &mut db,
        &root.path().join("main.pkl"),
        "import \"https://example.test/first.pkl\"",
    );
    let target_first = module(&mut db, &root.path().join("first/target.pkl"), "value = 1");
    let target_second = module(&mut db, &root.path().join("second/target.pkl"), "value = 2");
    let ImportResolution::Single(first) =
        resolve_import(&db, local, import_owner(&db, local), None)
    else {
        panic!("remote first target");
    };
    let ImportResolution::Single(second) = edge(&db, local, first, import_owner(&db, first)) else {
        panic!("remote descendant");
    };
    let owner = import_owner(&db, second);
    assert_eq!(
        edge(&db, local, second, owner),
        ImportResolution::Single(target_first)
    );
    db.set_projects(vec![project(root.path(), "second")]);
    for file in db.all_files() {
        let _ = pkl_db::import_graph(&db, file);
    }
    settle_inputs(&mut db);
    assert_eq!(
        edge(&db, local, second, owner),
        ImportResolution::Single(target_second)
    );
}

#[test]
fn missing_relative_modulepath_creation_resolves_without_settings_refresh() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = pkl_db::settings::WorkspaceSettings {
        modulepath: vec!["missing".into()],
        ..Default::default()
    };
    settings.resolve_paths(&[root.path().to_owned()]);
    let mut db = RootDatabase::new(Arc::new(StandardSourceLoader::default()), settings);
    let file = module(
        &mut db,
        &root.path().join("main.pkl"),
        "import \"modulepath:/target.pkl\"",
    );
    let owner = import_owner(&db, file);
    assert!(matches!(
        resolve_import(&db, file, owner, None),
        ImportResolution::Unresolved { .. }
    ));
    let path = root.path().join("missing/target.pkl");
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "value = 1").unwrap();
    refresh_inputs(&mut db);
    let uri = path_to_uri(&path).unwrap();
    let target = load_source(&mut db, &uri).unwrap();
    assert_eq!(
        resolve_import(&db, file, owner, None),
        ImportResolution::Single(target)
    );
}

#[test]
fn contextless_remote_and_two_project_roots_keep_separate_graph_edges() {
    let root = tempfile::tempdir().unwrap();
    let a_root = root.path().join("a");
    let b_root = root.path().join("b");
    let mut db = RootDatabase::default();
    db.set_projects(vec![
        project(&a_root, "modules"),
        project(&b_root, "modules"),
    ]);
    let remote_uri = Uri::parse("https://example.test/shared.pkl".to_owned()).unwrap();
    let remote = db
        .open_file(
            &remote_uri,
            Arc::from("import \"modulepath:/target.pkl\""),
            None,
        )
        .unwrap();
    let owner = import_owner(&db, remote);
    assert!(matches!(
        edge(&db, remote, remote, owner),
        ImportResolution::Unresolved { .. }
    ));
    let a = module(
        &mut db,
        &a_root.join("main.pkl"),
        "import \"https://example.test/shared.pkl\"",
    );
    let b = module(
        &mut db,
        &b_root.join("main.pkl"),
        "import \"https://example.test/shared.pkl\"",
    );
    let at = module(&mut db, &a_root.join("modules/target.pkl"), "value = 1");
    let bt = module(&mut db, &b_root.join("modules/target.pkl"), "value = 2");
    assert_eq!(edge(&db, a, remote, owner), ImportResolution::Single(at));
    assert_eq!(edge(&db, b, remote, owner), ImportResolution::Single(bt));
    assert_eq!(edge(&db, a, remote, owner), ImportResolution::Single(at));
    assert!(matches!(
        edge(&db, remote, remote, owner),
        ImportResolution::Unresolved { .. }
    ));
    assert!(pkl_db::project_for_file(&db, remote).is_none());
}

#[test]
fn local_transitive_project_overrides_importing_project_for_remote_descendants() {
    let root = tempfile::tempdir().unwrap();
    let outer = root.path().join("outer");
    let inner = outer.join("inner");
    let mut db = RootDatabase::default();
    db.set_projects(vec![project(&outer, "modules"), project(&inner, "modules")]);
    let remote_uri = Uri::parse("https://example.test/shared.pkl".to_owned()).unwrap();
    let remote = db
        .open_file(
            &remote_uri,
            Arc::from("import \"modulepath:/target.pkl\""),
            None,
        )
        .unwrap();
    let main = module(
        &mut db,
        &outer.join("main.pkl"),
        "import \"inner/child.pkl\"",
    );
    let child = module(
        &mut db,
        &inner.join("child.pkl"),
        "import \"https://example.test/shared.pkl\"",
    );
    let target = module(&mut db, &inner.join("modules/target.pkl"), "value = 1");
    module(&mut db, &outer.join("modules/target.pkl"), "value = 2");
    assert_eq!(
        edge(&db, main, main, import_owner(&db, main)),
        ImportResolution::Single(child)
    );
    assert_eq!(
        edge(&db, main, remote, import_owner(&db, remote)),
        ImportResolution::Single(target)
    );
}

#[test]
fn cyclic_import_graph_terminates_without_query_cycle() {
    let root = tempfile::tempdir().unwrap();
    let mut db = RootDatabase::default();
    let a = module(&mut db, &root.path().join("a.pkl"), "import \"b.pkl\"");
    let b = module(&mut db, &root.path().join("b.pkl"), "import \"a.pkl\"");
    assert_eq!(
        edge(&db, a, a, import_owner(&db, a)),
        ImportResolution::Single(b)
    );
    assert_eq!(
        edge(&db, a, b, import_owner(&db, b)),
        ImportResolution::Single(a)
    );
    assert_eq!(pkl_db::import_graph(&db, a).edges.len(), 2);
}

#[test]
fn missing_local_dependency_member_creation_resolves_without_metadata_refresh() {
    use pkl_project::dto::{
        Dependency, LocalDependency, PackageUri, ProjectDeps, ResolvedDependency,
        resolved_to_dependency,
    };
    let root = tempfile::tempdir().unwrap();
    let uri = PackageUri::create("package://example.test/local@1.0.0").unwrap();
    let resolved = ResolvedDependency::Local(LocalDependency {
        uri: uri.clone(),
        path: "missing".into(),
    });
    assert!(matches!(
        resolved_to_dependency(&resolved, root.path()),
        Dependency::LocalProject(_)
    ));
    let mut metadata = (*project(root.path(), "modules")).clone();
    metadata
        .metadata
        .declared_dependencies
        .insert("local".into(), uri.clone());
    metadata.project_deps = Some(ProjectDeps {
        schema_version: 1,
        resolved_dependencies: [(uri.to_display_string(), resolved)].into(),
    });
    let mut db = RootDatabase::default();
    db.set_projects(vec![Arc::new(metadata)]);
    let file = module(
        &mut db,
        &root.path().join("main.pkl"),
        "import \"@local/member.pkl\"",
    );
    let owner = import_owner(&db, file);
    assert_eq!(
        edge(&db, file, file, owner),
        ImportResolution::Unresolved {
            package_to_download: None
        }
    );
    let path = root.path().join("missing/member.pkl");
    let target_uri = path_to_uri(&path).unwrap();
    assert_eq!(
        db.known_file(&target_uri).unwrap().status(&db),
        FileStatus::Missing
    );
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "value = 1").unwrap();
    refresh_inputs(&mut db);
    let target = load_source(&mut db, &target_uri).unwrap();
    assert_eq!(
        edge(&db, file, file, owner),
        ImportResolution::Single(target)
    );
}

#[test]
fn one_graph_keeps_first_reachable_remote_context() {
    let root = tempfile::tempdir().unwrap();
    let inner = root.path().join("inner");
    let mut db = RootDatabase::default();
    db.set_projects(vec![
        project(root.path(), "modules"),
        project(&inner, "modules"),
    ]);
    let remote_uri = Uri::parse("https://example.test/shared.pkl".to_owned()).unwrap();
    let remote = db
        .open_file(
            &remote_uri,
            Arc::from("import \"modulepath:/target.pkl\""),
            None,
        )
        .unwrap();
    let main = module(
        &mut db,
        &root.path().join("main.pkl"),
        "import \"inner/child.pkl\"
import \"https://example.test/shared.pkl\"",
    );
    module(
        &mut db,
        &inner.join("child.pkl"),
        "import \"https://example.test/shared.pkl\"",
    );
    let inner_target = module(&mut db, &inner.join("modules/target.pkl"), "value = 1");
    let outer_target = module(
        &mut db,
        &root.path().join("modules/target.pkl"),
        "value = 2",
    );
    let owner = import_owner(&db, remote);
    assert_eq!(
        edge(&db, main, remote, owner),
        ImportResolution::Single(inner_target)
    );
    let main_uri = main.uri(&db).clone();
    db.upsert_file(
        &main_uri,
        Arc::from(
            "import \"https://example.test/shared.pkl\"
import \"inner/child.pkl\"",
        ),
        None,
        FileStatus::Present,
    )
    .unwrap();
    assert_eq!(
        edge(&db, main, remote, owner),
        ImportResolution::Single(outer_target)
    );
}

#[test]
fn graph_walks_amends_and_nested_import_expression_owners() {
    let root = tempfile::tempdir().unwrap();
    let mut db = RootDatabase::default();
    let parent = module(&mut db, &root.path().join("parent.pkl"), "value = 1");
    let target = module(&mut db, &root.path().join("target.pkl"), "value = 2");
    let main = module(
        &mut db,
        &root.path().join("main.pkl"),
        "amends \"parent.pkl\"
value = import(\"target.pkl\")",
    );
    let graph = pkl_db::import_graph(&db, main);
    assert_eq!(graph.edges.len(), 2);
    assert!(
        graph
            .edges
            .values()
            .any(|edge| *edge == ImportResolution::Single(parent))
    );
    assert!(
        graph
            .edges
            .values()
            .any(|edge| *edge == ImportResolution::Single(target))
    );
}

#[test]
fn unrelated_root_edit_keeps_contextual_graph_memoized() {
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
    let root = tempfile::tempdir().unwrap();
    let a = module(&mut db, &root.path().join("a.pkl"), "value = 1");
    let b = module(&mut db, &root.path().join("b.pkl"), "import \"target.pkl\"");
    let target = module(&mut db, &root.path().join("target.pkl"), "value = 2");
    pkl_db::import_graph(&db, a);
    assert_eq!(
        edge(&db, b, b, import_owner(&db, b)),
        ImportResolution::Single(target)
    );
    events.lock().clear();
    let uri = a.uri(&db).clone();
    db.upsert_file(&uri, Arc::from("value = 3"), None, FileStatus::Present)
        .unwrap();
    assert_eq!(
        edge(&db, b, b, import_owner(&db, b)),
        ImportResolution::Single(target)
    );
    assert!(events.lock().is_empty());
    pkl_db::import_graph(&db, a);
    let executions = events.lock();
    assert_eq!(
        executions
            .iter()
            .filter(|event| event.starts_with("import_graph("))
            .count(),
        1
    );
    assert_eq!(
        executions
            .iter()
            .filter(|event| event.starts_with("parse("))
            .count(),
        1
    );
    assert_eq!(
        executions
            .iter()
            .filter(|event| event.starts_with("line_index("))
            .count(),
        1
    );
    assert!(
        !executions
            .iter()
            .any(|event| event.starts_with("resolve_import("))
    );
}
