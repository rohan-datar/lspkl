//! Reachable import graphs and request-pinned syntax snapshots.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use fluent_uri::Uri;
use lspkl::ServerState;
use lspkl::state::AnalysisSnapshot;
use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};
use pkl_types::{AnalysisContext, ImportResolution, NodeRef};

fn uri(path: &Path) -> Uri<String> {
    Uri::parse(url::Url::from_file_path(path).unwrap().to_string()).unwrap()
}

fn prepare(state: &ServerState, path: &Path, source: &str) -> AnalysisSnapshot {
    let file = state.file_manager.open(uri(path), source.into(), Some(1));
    let module = state.parse_module(file).unwrap();
    state.prepare_analysis(module).unwrap()
}

#[test]
fn uri_diagnostics_use_real_resolution_for_single_expression_extends_glob_and_package() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("valid.pkl"), "value = 1").unwrap();
    let mut state = ServerState::new();
    state.package_cache_dir = Some(temp.path().join("cache"));
    let source = r#"extends "missing-parent.pkl"
import "valid.pkl" as valid
import "missing.pkl" as missing
import* "nothing-*.pkl" as empty
import "package://example.test/uncached@1.0.0#/main.pkl" as package
value = import("missing-expr.pkl")
"#;
    let snapshot = prepare(&state, &temp.path().join("main.pkl"), source);
    assert_eq!(snapshot.imports.len(), 6);
    assert_eq!(
        snapshot
            .imports
            .values()
            .filter(|edge| matches!(edge, ImportResolution::Single(_)))
            .count(),
        1
    );
    assert_eq!(
        snapshot
            .imports
            .values()
            .filter(|edge| matches!(edge, ImportResolution::Glob(targets) if targets.is_empty()))
            .count(),
        1
    );
    assert_eq!(
        snapshot
            .imports
            .values()
            .filter(|edge| matches!(
                edge,
                ImportResolution::Unresolved {
                    package_to_download: None
                }
            ))
            .count(),
        3
    );
    let ctx = AnalysisContext::new(
        &snapshot.module.tree,
        snapshot.module.file_id,
        state.base_module(),
        &snapshot.trees,
        &snapshot.imports,
    );
    let diagnostics = pkl_diagnostics::DiagnosticsEngine::new()
        .with(Box::new(pkl_diagnostics::analyzers::ModuleUriAnalyzer))
        .analyze(&ctx);
    assert_eq!(diagnostics.len(), 5);
    let downloads: Vec<_> = diagnostics.iter().flat_map(|d| &d.quick_fixes).collect();
    assert_eq!(downloads.len(), 1);
    assert_eq!(
        downloads[0].command,
        Some(
            pkl_diagnostics::diagnostic::QuickFixCommand::DownloadPackage {
                uri: "package://example.test/uncached@1.0.0".into()
            }
        )
    );
}

fn property_name(tree: &SyntaxTree) -> &str {
    let property = tree
        .child(tree.root(), |kind| matches!(kind, NodeKind::ClassProperty))
        .expect("fixture must contain a property");
    tree.name(property).unwrap()
}

#[test]
fn transitive_shared_dependencies_and_cycles_resolve_to_their_own_trees() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("workspace space");
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::create_dir_all(dir.join("shared")).unwrap();
    let main = "import \"./sub/b.pkl\"\nimport \"./sub/c.pkl\"\naValue = 1\n";
    fs::write(dir.join("main.pkl"), main).unwrap();
    fs::write(
        dir.join("sub/b.pkl"),
        "import \"../shared/d.pkl\"\nbValue = 2\n",
    )
    .unwrap();
    fs::write(
        dir.join("sub/c.pkl"),
        "import \"../shared/d.pkl\"\ncValue = 3\n",
    )
    .unwrap();
    fs::write(
        dir.join("shared/d.pkl"),
        "import \"../main.pkl\"\ndValue = 4\n",
    )
    .unwrap();
    let state = ServerState::new();
    let snapshot = prepare(&state, &dir.join("main.pkl"), main);
    let ctx = AnalysisContext::new(
        &snapshot.module.tree,
        snapshot.module.file_id,
        state.base_module(),
        &snapshot.trees,
        &snapshot.imports,
    );

    assert_eq!(snapshot.trees.len(), 4);
    let id_for = |path: &str| state.file_registry.lock().register(&uri(&dir.join(path)));
    let expected_edges = [
        ("main.pkl", 0, "sub/b.pkl"),
        ("main.pkl", 1, "sub/c.pkl"),
        ("sub/b.pkl", 0, "shared/d.pkl"),
        ("sub/c.pkl", 0, "shared/d.pkl"),
        ("shared/d.pkl", 0, "main.pkl"),
    ];
    assert_eq!(snapshot.imports.len(), expected_edges.len());
    for (source, index, target) in expected_edges {
        let source_id = id_for(source);
        let tree = &snapshot.trees[&source_id];
        let import = tree
            .children(tree.root())
            .iter()
            .copied()
            .filter(|&id| matches!(tree.kind(id), NodeKind::Import))
            .nth(index)
            .unwrap();
        assert_eq!(
            snapshot.imports.get(&NodeRef::new(source_id, import)),
            Some(&ImportResolution::Single(NodeRef::new(
                id_for(target),
                NodeId::ROOT
            ))),
            "edge from {source} to {target}",
        );
    }
    for (path, expected) in [
        ("main.pkl", "aValue"),
        ("sub/b.pkl", "bValue"),
        ("sub/c.pkl", "cValue"),
        ("shared/d.pkl", "dValue"),
    ] {
        let url = uri(&dir.join(path));
        let file = state.file_manager.get(&url).expect("reachable file loaded");
        let id = state.file_registry.lock().register(&url);
        let tree = ctx.tree_for(id).expect("reachable tree belongs to request");
        assert_eq!(property_name(tree), expected);
        assert!(Arc::ptr_eq(
            snapshot.trees.get(&id).unwrap(),
            &file.cached_tree().unwrap()
        ));
    }
    assert!(Arc::ptr_eq(
        snapshot.trees.get(&snapshot.module.file_id).unwrap(),
        &snapshot.module.tree
    ));
}

#[test]
fn reachable_imports_include_globs_and_record_unresolved_targets() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("good.pkl"), "goodValue = 1\n").unwrap();
    fs::write(temp.path().join("glob-only.pkl"), "globValue = 2\n").unwrap();
    let state = ServerState::new();
    state.file_manager.open(
        uri(&temp.path().join("unrelated.pkl")),
        "unrelatedValue = 3\n".into(),
        Some(1),
    );
    let snapshot = prepare(
        &state,
        &temp.path().join("main.pkl"),
        "import \"./good.pkl\"\nimport \"./missing.pkl\"\nimport \"ftp://example.com/remote.pkl\"\nimport* \"./glob-only.pkl\"\nmainValue = 0\n",
    );
    let mut names: Vec<_> = snapshot
        .trees
        .values()
        .map(|tree| property_name(tree))
        .collect();
    names.sort_unstable();

    assert_eq!(names, ["globValue", "goodValue", "mainValue"]);
    let good_id = state
        .file_registry
        .lock()
        .register(&uri(&temp.path().join("good.pkl")));
    let import = snapshot
        .module
        .tree
        .child(NodeId::ROOT, |kind| matches!(kind, NodeKind::Import))
        .unwrap();
    let glob_id = state
        .file_registry
        .lock()
        .register(&uri(&temp.path().join("glob-only.pkl")));
    let glob = snapshot
        .module
        .tree
        .child(NodeId::ROOT, |kind| matches!(kind, NodeKind::ImportGlob))
        .unwrap();
    let unresolved: Vec<_> = snapshot
        .module
        .tree
        .children_matching(NodeId::ROOT, |kind| matches!(kind, NodeKind::Import))
        .into_iter()
        .skip(1)
        .collect();
    assert_eq!(
        snapshot.imports,
        std::collections::HashMap::from([
            (
                NodeRef::new(snapshot.module.file_id, import),
                ImportResolution::Single(NodeRef::new(good_id, NodeId::ROOT))
            ),
            (
                NodeRef::new(snapshot.module.file_id, glob),
                ImportResolution::Glob(vec![NodeRef::new(glob_id, NodeId::ROOT)])
            ),
            (
                NodeRef::new(snapshot.module.file_id, unresolved[0]),
                ImportResolution::Unresolved {
                    package_to_download: None
                }
            ),
            (
                NodeRef::new(snapshot.module.file_id, unresolved[1]),
                ImportResolution::Unresolved {
                    package_to_download: None
                }
            ),
        ])
    );
}

#[test]
fn later_requests_see_editor_updates_without_replacing_previous_import_snapshots() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("library.pkl");
    fs::write(&target, "diskValue = 0\n").unwrap();
    let state = ServerState::new();
    let opened = state
        .file_manager
        .open(uri(&target), "oldValue = 1\n".into(), Some(1));
    let root = state.file_manager.open(
        uri(&temp.path().join("main.pkl")),
        "import \"./library.pkl\"\nmainValue = 0\n".into(),
        Some(1),
    );
    let before = state
        .prepare_analysis(state.parse_module(Arc::clone(&root)).unwrap())
        .unwrap();
    let imported_id = state.file_registry.lock().register(&opened.url);
    let before_ctx = AnalysisContext::new(
        &before.module.tree,
        before.module.file_id,
        state.base_module(),
        &before.trees,
        &before.imports,
    );
    assert_eq!(
        property_name(before_ctx.tree_for(imported_id).unwrap()),
        "oldValue"
    );

    state
        .file_manager
        .update(&opened.url, "newValue = 2\n".into(), Some(2))
        .unwrap();
    let after = state
        .prepare_analysis(state.parse_module(root).unwrap())
        .unwrap();
    let after_ctx = AnalysisContext::new(
        &after.module.tree,
        after.module.file_id,
        state.base_module(),
        &after.trees,
        &after.imports,
    );

    assert_eq!(
        property_name(after_ctx.tree_for(imported_id).unwrap()),
        "newValue"
    );
    assert_eq!(
        property_name(before_ctx.tree_for(imported_id).unwrap()),
        "oldValue"
    );
    assert_eq!(before.module.file_id, after.module.file_id);
    assert!(Arc::ptr_eq(&before.module.tree, &after.module.tree));
    assert!(!Arc::ptr_eq(
        &before.trees[&imported_id],
        &after.trees[&imported_id]
    ));
}
