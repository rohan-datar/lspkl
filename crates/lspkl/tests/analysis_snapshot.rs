//! Reachable import graphs and request-pinned syntax snapshots.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use fluent_uri::Uri;
use lspkl::Workspace;
use lspkl::state::AnalysisSnapshot;
use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};
use pkl_types::{ImportResolution, NodeRef};

fn uri(path: &Path) -> Uri<String> {
    Uri::parse(url::Url::from_file_path(path).unwrap().to_string()).unwrap()
}

fn prepare(state: &mut Workspace, path: &Path, source: &str) -> AnalysisSnapshot {
    let file = open(state, uri(path), source.into(), Some(1));
    let module = state.snapshot().parse_module(file).unwrap();
    state.snapshot().prepare_analysis(module).unwrap()
}

#[test]
fn sdk_document_lifecycle_keeps_builtin_metadata_and_pinned_sources_consistent() {
    for warm in [false, true] {
        let mut state = Workspace::new();
        if warm {
            let _ = state.snapshot().base_module();
        }
        let url = Uri::parse("pkl:base".to_owned()).unwrap();
        let file = open(&mut state, url.clone(), "edited = 1\n".into(), Some(1));
        let view = state.snapshot();
        let before = view
            .prepare_analysis(view.parse_module(file).unwrap())
            .unwrap();
        assert_eq!(
            view.base_module()
                .string_type()
                .render(&before.context(view.base_module())),
            "String"
        );
        assert_eq!(before.module.tree.source(), before.module.file.contents());
        change(&mut state, &url, "edited = 2\n", 2);
        let current = state.snapshot();
        let changed = current
            .open_document_analysis(url.as_str())
            .unwrap()
            .unwrap();
        assert_eq!(
            current
                .base_module()
                .string_type()
                .render(&changed.context(current.base_module())),
            "String"
        );
        assert_eq!(changed.module.tree.source(), changed.module.file.contents());
        let workspace = tempfile::tempdir().unwrap();
        let imported = prepare(
            &mut state,
            &workspace.path().join("main.pkl"),
            "import \"pkl:base\" as sdk\nresult: sdk.String = \"value\"\n",
        );
        let imported_view = state.snapshot();
        let ctx = imported.context(imported_view.base_module());
        let property = imported
            .module
            .tree
            .child(imported.module.tree.root(), |kind| {
                matches!(kind, NodeKind::ClassProperty)
            })
            .unwrap();
        assert_eq!(
            pkl_types::resolver::definition_type(
                NodeRef::new(imported.module.file_id, property),
                &ctx
            )
            .render(&ctx),
            "String"
        );
        assert_eq!(
            ctx.tree(imported_view.base_module().file_id())
                .unwrap()
                .source(),
            changed.module.file.contents()
        );
        state
            .apply_close(lsp_types::DidCloseTextDocumentParams {
                text_document: lsp_types::TextDocumentIdentifier {
                    uri: url.as_str().parse().unwrap(),
                },
            })
            .unwrap();
        let closed = state.snapshot();
        let restored = closed
            .parse_module(closed.file_manager.get_or_load(&url).unwrap())
            .unwrap();
        let after = closed.prepare_analysis(restored).unwrap();
        assert_eq!(
            closed
                .base_module()
                .string_type()
                .render(&after.context(closed.base_module())),
            "String"
        );
        assert_eq!(
            view.base_module()
                .string_type()
                .render(&before.context(view.base_module())),
            "String"
        );
        assert_eq!(before.module.file.version(), Some(1));
        assert_eq!(changed.module.file.version(), Some(2));
    }
}

#[test]
fn uri_diagnostics_use_real_resolution_for_single_expression_extends_glob_and_package() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("valid.pkl"), "value = 1").unwrap();
    let mut state = Workspace::new();
    state.apply_settings(
        lspkl::settings::WorkspaceSettings::parse(
            serde_json::json!({ "cacheDir": temp.path().join("cache") }),
        )
        .unwrap(),
    );
    let source = r#"extends "missing-parent.pkl"
import "valid.pkl" as valid
import "missing.pkl" as missing
import* "nothing-*.pkl" as empty
import "package://example.test/uncached@1.0.0#/main.pkl" as package
value = import("missing-expr.pkl")
"#;
    let snapshot = prepare(&mut state, &temp.path().join("main.pkl"), source);
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
    let view = state.snapshot();
    let ctx = snapshot.context(view.base_module());
    let diagnostics = pkl_diagnostics::DiagnosticsEngine::new()
        .with(Box::new(pkl_diagnostics::analyzers::ModuleUriAnalyzer))
        .analyze(&ctx, snapshot.module.file_id);
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
    let mut state = Workspace::new();
    let snapshot = prepare(&mut state, &dir.join("main.pkl"), main);
    let view = state.snapshot();
    let ctx = snapshot.context(view.base_module());

    assert_eq!(view.base_module().string_type().render(&ctx), "String");
    let id_for = |path: &str| {
        state
            .snapshot()
            .file_registry
            .lock()
            .register(&uri(&dir.join(path)))
    };
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
        let file = state
            .snapshot()
            .file_manager
            .get(&url)
            .expect("reachable file loaded");
        let id = state.snapshot().file_registry.lock().register(&url);
        let tree = ctx.tree(id).expect("reachable tree belongs to request");
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
    let mut state = Workspace::new();
    open(
        &mut state,
        uri(&temp.path().join("unrelated.pkl")),
        "unrelatedValue = 3\n".into(),
        Some(1),
    );
    let snapshot = prepare(
        &mut state,
        &temp.path().join("main.pkl"),
        "import \"./good.pkl\"\nimport \"./missing.pkl\"\nimport \"ftp://example.com/remote.pkl\"\nimport* \"./glob-only.pkl\"\nmainValue = 0\n",
    );
    let view = state.snapshot();
    for (path, expected) in [
        ("main.pkl", "mainValue"),
        ("good.pkl", "goodValue"),
        ("glob-only.pkl", "globValue"),
    ] {
        let id = view
            .file_registry
            .lock()
            .register(&uri(&temp.path().join(path)));
        assert_eq!(
            property_name(snapshot.trees.get(&id).expect("reachable module pinned")),
            expected
        );
    }
    let unrelated = view
        .file_registry
        .lock()
        .register(&uri(&temp.path().join("unrelated.pkl")));
    assert!(!snapshot.trees.contains_key(&unrelated));
    let good_id = state
        .snapshot()
        .file_registry
        .lock()
        .register(&uri(&temp.path().join("good.pkl")));
    let import = snapshot
        .module
        .tree
        .child(NodeId::ROOT, |kind| matches!(kind, NodeKind::Import))
        .unwrap();
    let glob_id = state
        .snapshot()
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
    let mut state = Workspace::new();
    let opened = open(&mut state, uri(&target), "oldValue = 1\n".into(), Some(1));
    let root = open(
        &mut state,
        uri(&temp.path().join("main.pkl")),
        "import \"./library.pkl\"\nmainValue = 0\n".into(),
        Some(1),
    );
    let before = state
        .snapshot()
        .prepare_analysis(state.snapshot().parse_module(Arc::clone(&root)).unwrap())
        .unwrap();
    let imported_id = state.snapshot().file_registry.lock().register(&opened.url);
    let view = state.snapshot();
    let before_ctx = before.context(view.base_module());
    assert_eq!(
        property_name(before_ctx.tree(imported_id).unwrap()),
        "oldValue"
    );

    change(&mut state, &opened.url, "newValue = 2\n", 2);
    let after = state
        .snapshot()
        .prepare_analysis(state.snapshot().parse_module(root).unwrap())
        .unwrap();
    let view = state.snapshot();
    let after_ctx = after.context(view.base_module());

    assert_eq!(
        property_name(after_ctx.tree(imported_id).unwrap()),
        "newValue"
    );
    assert_eq!(
        property_name(before_ctx.tree(imported_id).unwrap()),
        "oldValue"
    );
    assert_eq!(before.module.file_id, after.module.file_id);
    assert!(Arc::ptr_eq(&before.module.tree, &after.module.tree));
    assert!(!Arc::ptr_eq(
        &before.trees[&imported_id],
        &after.trees[&imported_id]
    ));
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

#[test]
fn captured_buffers_drive_import_analysis_even_after_edit_close_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root_uri = uri(&temp.path().join("main.pkl"));
    let target_uri = uri(&temp.path().join("library.pkl"));
    let mut state = Workspace::new();
    open(
        &mut state,
        root_uri.clone(),
        r#"import "./library.pkl"
rootBefore = 1
"#
        .into(),
        Some(1),
    );
    open(
        &mut state,
        target_uri.clone(),
        "importBefore = 1
"
        .into(),
        Some(1),
    );
    let captured = state.snapshot();
    change(
        &mut state,
        &root_uri,
        r#"import "./library.pkl"
rootAfter = 2
"#,
        2,
    );
    change(
        &mut state,
        &target_uri,
        "importAfter = 2
",
        2,
    );
    state
        .apply_close(lsp_types::DidCloseTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier {
                uri: target_uri.as_str().parse().unwrap(),
            },
        })
        .unwrap();
    open(
        &mut state,
        target_uri.clone(),
        "reopened = 3
"
        .into(),
        Some(3),
    );
    // Parse for the first time after all writes: no pre-parsed graph hides epoch leaks.
    let old = captured
        .open_document_analysis(root_uri.as_str())
        .unwrap()
        .unwrap();
    let new = state
        .snapshot()
        .open_document_analysis(root_uri.as_str())
        .unwrap()
        .unwrap();
    let target_id = captured.file_registry.lock().register(&target_uri);
    assert_eq!(property_name(&old.module.tree), "rootBefore");
    assert_eq!(property_name(&old.trees[&target_id]), "importBefore");
    assert_eq!(old.module.file.version(), Some(1));
    assert_eq!(
        captured.document(target_uri.as_str()).unwrap().version(),
        Some(1)
    );
    assert_eq!(property_name(&new.module.tree), "rootAfter");
    assert_eq!(property_name(&new.trees[&target_id]), "reopened");
}

#[test]
fn closing_a_saved_import_reloads_disk_instead_of_a_pre_open_cache_entry() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("library.pkl");
    let target_uri = uri(&target);
    let root_uri = uri(&temp.path().join("main.pkl"));
    fs::write(&target, "diskBefore = 0\n").unwrap();
    let mut workspace = Workspace::new();
    let before = prepare(
        &mut workspace,
        &temp.path().join("main.pkl"),
        "import \"./library.pkl\"\nroot = 1\n",
    );
    let target_id = workspace
        .snapshot()
        .file_registry
        .lock()
        .register(&target_uri);
    assert_eq!(property_name(&before.trees[&target_id]), "diskBefore");
    open(
        &mut workspace,
        target_uri.clone(),
        "savedAfter = 2\n".into(),
        Some(1),
    );
    let captured = workspace.snapshot();
    fs::write(&target, "savedAfter = 2\n").unwrap();
    workspace
        .apply_save(lsp_types::DidSaveTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier {
                uri: target_uri.as_str().parse().unwrap(),
            },
            text: None,
        })
        .unwrap();
    workspace
        .apply_close(lsp_types::DidCloseTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier {
                uri: target_uri.as_str().parse().unwrap(),
            },
        })
        .unwrap();
    let after = workspace
        .snapshot()
        .open_document_analysis(root_uri.as_str())
        .unwrap()
        .unwrap();
    assert_eq!(property_name(&after.trees[&target_id]), "savedAfter");
    let pinned = captured
        .open_document_analysis(root_uri.as_str())
        .unwrap()
        .unwrap();
    assert_eq!(property_name(&pinned.trees[&target_id]), "savedAfter");
    assert_eq!(property_name(&before.trees[&target_id]), "diskBefore");
}
