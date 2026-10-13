//! Database-backed import graphs and completed request observations.
use fluent_uri::Uri;
use lspkl::Workspace;
use pkl_db::{Db, File, FileStatus, ImportResolution, parse, resolve_import};
use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};
use pkl_types::NodeRef;
use std::{fs, path::Path, sync::Arc};

fn uri(path: &Path) -> Uri<String> {
    pkl_vfs::path_to_uri(path).unwrap()
}
fn prepare(state: &mut Workspace, path: &Path, source: &str) -> File {
    open(state, uri(path), source.into(), Some(1))
}
fn owners(tree: &SyntaxTree) -> Vec<NodeId> {
    let mut pending = vec![tree.root()];
    let mut result = Vec::new();
    while let Some(node) = pending.pop() {
        if tree.kind(node).is_module_uri_owner() {
            result.push(node);
        }
        pending.extend(tree.children(node).iter().rev().copied());
    }
    result
}
fn property_name(tree: &SyntaxTree) -> &str {
    let property = tree
        .child(tree.root(), |kind| matches!(kind, NodeKind::ClassProperty))
        .unwrap();
    tree.name(property).unwrap()
}
fn single(state: &Workspace, file: File) -> File {
    let tree = parse(&state.db, file).as_ref().unwrap();
    let node = tree
        .child(tree.root(), |kind| matches!(kind, NodeKind::Import))
        .unwrap();
    match resolve_import(&state.db, file, node, None) {
        ImportResolution::Single(target) => target,
        other => panic!("expected single import: {other:?}"),
    }
}
fn close(state: &mut Workspace, url: &Uri<String>) {
    let work = state
        .apply_close(lsp_types::DidCloseTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier {
                uri: url.as_str().parse().unwrap(),
            },
        })
        .unwrap();
    assert!(state.document(url.as_str()).is_none());
    state.apply_reload(work.load());
}

#[test]
fn sdk_document_lifecycle_keeps_builtin_identity_and_metadata_immutable() {
    for warm in [false, true] {
        let mut state = Workspace::new();
        let url = Uri::parse("pkl:base".to_owned()).unwrap();
        let builtin = state.db.file(&url).unwrap();
        let original = parse(&state.db, builtin).as_ref().unwrap().clone();
        if warm {
            let view = state.snapshot();
            assert_eq!(view.base_module().file_id(), builtin);
        }
        let file = open(&mut state, url.clone(), "edited = 1\n".into(), Some(1));
        assert_eq!(file, builtin);
        assert_eq!(file.version(&state.db), Some(1));
        {
            let view = state.snapshot();
            let ctx = view.context(file);
            assert_eq!(ctx.base.file_id(), file);
            assert_eq!(ctx.base.string_type().render(&ctx), "String");
            assert_eq!(ctx.tree(file).unwrap().source(), original.source());
        }
        change(&mut state, &url, "edited = 2\n", 2);
        assert_eq!(file.version(&state.db), Some(2));
        assert!(Arc::ptr_eq(
            &original,
            parse(&state.db, file).as_ref().unwrap()
        ));
        let temp = tempfile::tempdir().unwrap();
        let root = prepare(
            &mut state,
            &temp.path().join("main.pkl"),
            "import \"pkl:base\" as sdk\nresult: sdk.String = \"value\"\n",
        );
        {
            let view = state.snapshot();
            let ctx = view.context(root);
            assert_eq!(single(&state, root), builtin);
            let tree = ctx.tree(root).unwrap();
            let property = tree
                .child(tree.root(), |kind| matches!(kind, NodeKind::ClassProperty))
                .unwrap();
            assert_eq!(
                pkl_types::resolver::definition_type(NodeRef::new(root, property), &ctx)
                    .render(&ctx),
                "String"
            );
            assert_eq!(ctx.tree(builtin).unwrap().source(), original.source());
        }
        close(&mut state, &url);
        assert_eq!(file.status(&state.db), FileStatus::Present);
        assert_eq!(file.version(&state.db), None);
        assert_eq!(state.db.file(&url).unwrap(), builtin);
        let view = state.snapshot();
        let ctx = view.context(root);
        assert_eq!(ctx.base.string_type().render(&ctx), "String");
        assert_eq!(ctx.tree(builtin).unwrap().source(), original.source());
    }
}
#[test]
fn uri_diagnostics_use_real_resolution_for_single_expression_extends_glob_and_package() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("valid.pkl"), "value = 1").unwrap();
    let mut state = Workspace::new();
    state.apply_settings(
        pkl_db::settings::WorkspaceSettings::parse(
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
    let root = prepare(&mut state, &temp.path().join("main.pkl"), source);
    let view = state.snapshot();
    let tree = parse(&view.db, root).as_ref().unwrap();
    let edges: Vec<_> = owners(tree)
        .into_iter()
        .map(|owner| resolve_import(&view.db, root, owner, None))
        .collect();
    assert_eq!(edges.len(), 6);
    assert_eq!(
        edges
            .iter()
            .filter(|edge| matches!(edge, ImportResolution::Single(_)))
            .count(),
        1
    );
    assert_eq!(
        edges
            .iter()
            .filter(|edge| matches!(edge, ImportResolution::Glob(targets) if targets.is_empty()))
            .count(),
        1
    );
    assert_eq!(
        edges
            .iter()
            .filter(|edge| matches!(
                edge,
                ImportResolution::Unresolved {
                    package_to_download: None
                }
            ))
            .count(),
        3
    );
    let ctx = view.context(root);
    let diagnostics = pkl_diagnostics::DiagnosticsEngine::new()
        .with(Box::new(pkl_diagnostics::analyzers::ModuleUriAnalyzer))
        .analyze(&ctx, root);
    assert_eq!(diagnostics.len(), 5);
    let downloads: Vec<_> = diagnostics.iter().flat_map(|d| &d.quick_fixes).collect();
    assert_eq!(downloads.len(), 1);
    assert_eq!(
        downloads[0].command,
        Some(
            pkl_diagnostics::diagnostic::QuickFixCommand::DownloadPackage {
                uri: "package://example.test/uncached@1.0.0".into(),
            }
        )
    );
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
    let root = prepare(&mut state, &dir.join("main.pkl"), main);
    let view = state.snapshot();
    let ctx = view.context(root);
    assert_eq!(ctx.base.string_type().render(&ctx), "String");
    let file_for = |path: &str| view.db.file(&uri(&dir.join(path))).unwrap();
    let expected_edges = [
        ("main.pkl", 0, "sub/b.pkl"),
        ("main.pkl", 1, "sub/c.pkl"),
        ("sub/b.pkl", 0, "shared/d.pkl"),
        ("sub/c.pkl", 0, "shared/d.pkl"),
        ("shared/d.pkl", 0, "main.pkl"),
    ];
    for (source, index, target) in expected_edges {
        let file = file_for(source);
        let tree = ctx.tree(file).unwrap();
        let owner =
            tree.children_matching(tree.root(), |kind| matches!(kind, NodeKind::Import))[index];
        assert_eq!(
            resolve_import(&view.db, file, owner, None),
            ImportResolution::Single(file_for(target)),
            "edge from {source} to {target}"
        );
        assert_eq!(
            ctx.import(NodeRef::new(file, owner)),
            Some(pkl_types::ImportResolution::Single(NodeRef::new(
                file_for(target),
                NodeId::ROOT
            )))
        );
    }
    assert_eq!(file_for("main.pkl"), root);
    for (path, expected) in [
        ("main.pkl", "aValue"),
        ("sub/b.pkl", "bValue"),
        ("sub/c.pkl", "cValue"),
        ("shared/d.pkl", "dValue"),
    ] {
        let file = file_for(path);
        let tree = ctx.tree(file).unwrap();
        assert_eq!(property_name(tree), expected);
        assert_eq!(tree.source(), file.text(&view.db).as_ref());
    }
}

#[test]
fn reachable_imports_include_globs_and_record_unresolved_targets() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("good.pkl"), "goodValue = 1\n").unwrap();
    fs::write(temp.path().join("glob-only.pkl"), "globValue = 2\n").unwrap();
    let mut state = Workspace::new();
    let unrelated = open(
        &mut state,
        uri(&temp.path().join("unrelated.pkl")),
        "unrelatedValue = 3\n".into(),
        Some(1),
    );
    let root = prepare(
        &mut state,
        &temp.path().join("main.pkl"),
        "import \"./good.pkl\"\nimport \"./missing.pkl\"\nimport \"ftp://example.com/remote.pkl\"\nimport* \"./glob-only.pkl\"\nmainValue = 0\n",
    );
    let view = state.snapshot();
    let ctx = view.context(root);
    let good = view.db.file(&uri(&temp.path().join("good.pkl"))).unwrap();
    let glob = view
        .db
        .file(&uri(&temp.path().join("glob-only.pkl")))
        .unwrap();
    let tree = ctx.tree(root).unwrap();
    let edges: Vec<_> = owners(tree)
        .into_iter()
        .map(|owner| resolve_import(&view.db, root, owner, None))
        .collect();
    assert_eq!(
        edges,
        vec![
            ImportResolution::Single(good),
            ImportResolution::Unresolved {
                package_to_download: None
            },
            ImportResolution::Unresolved {
                package_to_download: None
            },
            ImportResolution::Glob(vec![glob])
        ]
    );
    assert!(!edges.iter().any(|edge| match edge {
        ImportResolution::Single(file) => *file == unrelated,
        ImportResolution::Glob(files) => files.contains(&unrelated),
        _ => false,
    }));
    for (file, name) in [
        (root, "mainValue"),
        (good, "goodValue"),
        (glob, "globValue"),
    ] {
        assert_eq!(property_name(ctx.tree(file).unwrap()), name);
    }
}

#[test]
fn later_requests_see_editor_updates_without_mutating_owned_tree_observations() {
    let temp = tempfile::tempdir().unwrap();
    let target_uri = uri(&temp.path().join("library.pkl"));
    fs::write(temp.path().join("library.pkl"), "diskValue = 0\n").unwrap();
    let mut state = Workspace::new();
    let target = open(
        &mut state,
        target_uri.clone(),
        "oldValue = 1\n".into(),
        Some(1),
    );
    let root = prepare(
        &mut state,
        &temp.path().join("main.pkl"),
        "import \"./library.pkl\"\nmainValue = 0\n",
    );
    let (before_root, before_target) = {
        let view = state.snapshot();
        assert_eq!(single(&state, root), target);
        let ctx = view.context(root);
        assert_eq!(property_name(ctx.tree(target).unwrap()), "oldValue");
        (
            parse(&view.db, root).as_ref().unwrap().clone(),
            parse(&view.db, target).as_ref().unwrap().clone(),
        )
    };
    change(&mut state, &target_uri, "newValue = 2\n", 2);
    let view = state.snapshot();
    assert_eq!(single(&state, root), target);
    assert_eq!(
        property_name(view.context(root).tree(target).unwrap()),
        "newValue"
    );
    assert_eq!(property_name(&before_target), "oldValue");
    assert!(Arc::ptr_eq(
        &before_root,
        parse(&view.db, root).as_ref().unwrap()
    ));
    assert!(!Arc::ptr_eq(
        &before_target,
        parse(&view.db, target).as_ref().unwrap()
    ));
}

#[test]
fn completed_consumers_observe_edit_close_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root_uri = uri(&temp.path().join("main.pkl"));
    let target_uri = uri(&temp.path().join("library.pkl"));
    let mut state = Workspace::new();
    let root = open(
        &mut state,
        root_uri.clone(),
        "import \"./library.pkl\"\nrootBefore = 1\n".into(),
        Some(1),
    );
    let target = open(
        &mut state,
        target_uri.clone(),
        "importBefore = 1\n".into(),
        Some(1),
    );
    let observed = {
        let view = state.snapshot();
        assert_eq!(single(&state, root), target);
        let ctx = view.context(root);
        (
            property_name(ctx.tree(root).unwrap()).to_owned(),
            property_name(ctx.tree(target).unwrap()).to_owned(),
            view.document_version(root_uri.as_str()),
            view.document_version(target_uri.as_str()),
        )
    };
    assert_eq!(
        observed,
        ("rootBefore".into(), "importBefore".into(), Some(1), Some(1))
    );
    change(
        &mut state,
        &root_uri,
        "import \"./library.pkl\"\nrootAfter = 2\n",
        2,
    );
    change(&mut state, &target_uri, "importAfter = 2\n", 2);
    {
        let view = state.snapshot();
        assert_eq!(
            property_name(view.context(root).tree(target).unwrap()),
            "importAfter"
        );
        assert_eq!(view.document_version(target_uri.as_str()), Some(2));
    }
    close(&mut state, &target_uri);
    assert!(matches!(
        resolve_import(
            &state.db,
            root,
            owners(parse(&state.db, root).as_ref().unwrap())[0],
            None
        ),
        ImportResolution::Unresolved { .. }
    ));
    let reopened = open(
        &mut state,
        target_uri.clone(),
        "reopened = 3\n".into(),
        Some(3),
    );
    assert_eq!(reopened, target);
    let view = state.snapshot();
    assert_eq!(single(&state, root), target);
    assert_eq!(
        property_name(view.context(root).tree(root).unwrap()),
        "rootAfter"
    );
    assert_eq!(
        property_name(view.context(root).tree(target).unwrap()),
        "reopened"
    );
    assert_eq!(view.document_version(target_uri.as_str()), Some(3));
}

#[test]
fn closing_a_saved_import_reloads_disk_instead_of_a_pre_open_source() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.pkl");
    let target_uri = uri(&path);
    fs::write(&path, "diskBefore = 0\n").unwrap();
    let mut state = Workspace::new();
    let root = prepare(
        &mut state,
        &temp.path().join("main.pkl"),
        "import \"./library.pkl\"\nroot = 1\n",
    );
    let target = single(&state, root);
    let before = parse(&state.db, target).as_ref().unwrap().clone();
    assert_eq!(property_name(&before), "diskBefore");
    assert_eq!(
        open(
            &mut state,
            target_uri.clone(),
            "savedAfter = 2\n".into(),
            Some(1)
        ),
        target
    );
    let editor = {
        let view = state.snapshot();
        assert_eq!(single(&state, root), target);
        parse(&view.db, target).as_ref().unwrap().clone()
    };
    fs::write(&path, "savedAfter = 2\n").unwrap();
    state
        .apply_save(lsp_types::DidSaveTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier {
                uri: target_uri.as_str().parse().unwrap(),
            },
            text: None,
        })
        .unwrap();
    close(&mut state, &target_uri);
    assert_eq!(single(&state, root), target);
    assert_eq!(target.status(&state.db), FileStatus::Present);
    assert_eq!(target.version(&state.db), None);
    assert_eq!(
        property_name(parse(&state.db, target).as_ref().unwrap()),
        "savedAfter"
    );
    assert_eq!(property_name(&editor), "savedAfter");
    assert_eq!(property_name(&before), "diskBefore");
}

fn open(
    state: &mut Workspace,
    url: Uri<String>,
    text: String,
    version: Option<i32>,
) -> pkl_db::File {
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
    state.document(url.as_str()).unwrap()
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

fn canonical_uri(text: &str) -> Uri<String> {
    Uri::parse(text.to_owned()).unwrap()
}

#[test]
fn distinct_uris_with_the_same_basename_have_distinct_non_base_files() {
    let state = Workspace::new();
    let first_uri = canonical_uri("file:///workspace/first/library.pkl");
    let second_uri = canonical_uri("file:///workspace/second/library.pkl");
    let first = state.db.file(&first_uri).unwrap();
    let second = state.db.file(&second_uri).unwrap();
    let base = state.db.file(&canonical_uri("pkl:base")).unwrap();
    assert_ne!(first, base);
    assert_ne!(second, base);
    assert_ne!(first, second);
    assert_eq!(first.uri(&state.db), &first_uri);
    assert_eq!(second.uri(&state.db), &second_uri);
    assert_eq!(
        state.db.file(&canonical_uri(first_uri.as_str())).unwrap(),
        first
    );
}

#[test]
fn editor_changes_do_not_change_the_file_identity() {
    let mut state = Workspace::new();
    let url = canonical_uri("file:///workspace/library.pkl");
    let before = open(&mut state, url.clone(), "oldValue = 1\n".into(), Some(1));
    change(&mut state, &url, "newValue = 2\n", 2);
    let after = state.document(url.as_str()).unwrap();
    assert_eq!(after, before);
    assert_eq!(after.uri(&state.db), &url);
    assert_eq!(after.version(&state.db), Some(2));
    assert_eq!(
        parse(&state.db, after).as_ref().unwrap().source(),
        "newValue = 2\n"
    );
}

#[test]
fn closing_and_reopening_preserves_identity_without_reusing_it_for_another_canonical_uri() {
    let mut state = Workspace::new();
    let url = canonical_uri("file:///workspace/library.pkl");
    let other_url = canonical_uri("file:///workspace/other.pkl");
    let original = open(&mut state, url.clone(), "value = 1\n".into(), Some(1));
    let work = state
        .apply_close(lsp_types::DidCloseTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier {
                uri: url.as_str().parse().unwrap(),
            },
        })
        .unwrap();
    assert!(state.document(url.as_str()).is_none());
    assert_eq!(original.version(&state.db), None);
    assert_eq!(original.status(&state.db), FileStatus::Missing);
    state.apply_reload(work.load());
    assert_eq!(original.uri(&state.db), &url);
    let other = state.db.file(&other_url).unwrap();
    assert_ne!(other, original);
    assert_eq!(other.uri(&state.db), &other_url);
    let reopened = open(&mut state, url.clone(), "value = 2\n".into(), Some(2));
    assert_eq!(reopened, original);
    assert_eq!(reopened.text(&state.db).as_ref(), "value = 2\n");
}

#[test]
fn base_is_addressable_like_every_other_file() {
    let state = Workspace::new();
    let base_uri = canonical_uri("pkl:base");
    let base = state.db.file(&base_uri).unwrap();
    assert_eq!(base.uri(&state.db), &base_uri);
    assert_eq!(base.status(&state.db), FileStatus::Present);
    assert!(parse(&state.db, base).is_ok());
    let view = state.snapshot();
    assert_eq!(view.context(base).base.file_id(), base);
    assert_ne!(
        state
            .db
            .file(&canonical_uri("file:///workspace/main.pkl"))
            .unwrap(),
        base
    );
}
