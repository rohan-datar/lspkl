//! AST-to-file import resolution using real syntax trees and temporary workspaces.

mod support;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use fluent_uri::Uri;
use lspkl::Workspace;
use pkl_db::{File, ImportResolution, parse, resolve_import};
use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};

fn file_uri(path: &Path) -> Uri<String> {
    Uri::parse(url::Url::from_file_path(path).unwrap().to_string()).unwrap()
}

fn source(state: &mut Workspace, path: &Path, text: &str) -> (File, Arc<SyntaxTree>) {
    let file = open(state, file_uri(path), text.into(), Some(1));
    let tree = parse(state.database(), file).as_ref().unwrap().clone();
    (file, tree)
}

fn import_node(tree: &SyntaxTree) -> NodeId {
    tree.child(tree.root(), |kind| matches!(kind, NodeKind::Import))
        .expect("fixture must contain an ordinary import")
}

fn load_projects(state: &mut Workspace, workspace: &Path, projects: &[(&str, &[&str])]) {
    let entries: Vec<_> = projects
        .iter()
        .map(|(project_file, roots)| {
            serde_json::json!({
                "projectFile": project_file,
                "packageUri": null,
                "declaredDependencies": {},
                "resolvedDependencies": null,
                "evaluatorSettings": { "moduleCacheDir": null, "modulePath": roots }
            })
        })
        .collect();
    let state_file = workspace.join(".lspkl/projects.json");
    fs::create_dir_all(state_file.parent().unwrap()).unwrap();
    fs::write(
        &state_file,
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "pklProjects": entries
        }))
        .unwrap(),
    )
    .unwrap();
    state.apply_initialize(&lsp_types::InitializeParams {
        workspace_folders: Some(vec![lsp_types::WorkspaceFolder {
            uri: file_uri(workspace).as_str().parse().unwrap(),
            name: "fixture".into(),
        }]),
        ..Default::default()
    });
    let mut manager = state.project_manager();
    manager.load_state(&state_file).unwrap();
    state.apply_project_manager(manager);
}

#[test]
fn relative_import_uses_ast_literal_and_source_directory_without_a_project() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("source directory");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("library.pkl"), "answer = 42\n").unwrap();
    let mut state = Workspace::new();
    let (source, tree) = source(
        &mut state,
        &dir.join("main.pkl"),
        "// import \"./not-the-target.pkl\"\nimport \"./library.pkl\"\n",
    );

    let target = match resolve_import(state.database(), source, import_node(&tree), None) {
        ImportResolution::Single(file) => file,
        other => panic!("expected single import: {other:?}"),
    };

    assert_eq!(
        *target.uri(state.database()),
        file_uri(&dir.join("library.pkl"))
    );
    assert_eq!(target.text(state.database()).as_ref(), "answer = 42\n");
}

#[test]
fn modulepath_import_selects_the_nearest_project_for_each_source() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path();
    fs::create_dir_all(workspace.join("outer-lib")).unwrap();
    fs::create_dir_all(workspace.join("app/inner-lib")).unwrap();
    fs::write(
        workspace.join("outer-lib/library.pkl"),
        "owner = \"outer\"\n",
    )
    .unwrap();
    fs::write(
        workspace.join("app/inner-lib/library.pkl"),
        "owner = \"inner\"\n",
    )
    .unwrap();
    let mut state = Workspace::new();
    load_projects(
        &mut state,
        workspace,
        &[
            ("PklProject", &["outer-lib"]),
            ("app/PklProject", &["inner-lib"]),
        ],
    );
    let (outer, outer_tree) = source(
        &mut state,
        &workspace.join("main.pkl"),
        "import \"modulepath:/library.pkl\"\n",
    );
    let (inner, inner_tree) = source(
        &mut state,
        &workspace.join("app/main.pkl"),
        "import \"modulepath:/library.pkl\"\n",
    );

    let outer_target = match resolve_import(state.database(), outer, import_node(&outer_tree), None)
    {
        ImportResolution::Single(file) => file,
        other => panic!("expected single import: {other:?}"),
    };
    let inner_target = match resolve_import(state.database(), inner, import_node(&inner_tree), None)
    {
        ImportResolution::Single(file) => file,
        other => panic!("expected single import: {other:?}"),
    };

    assert_eq!(
        *outer_target.uri(state.database()),
        file_uri(&workspace.join("outer-lib/library.pkl"))
    );
    assert_eq!(
        *inner_target.uri(state.database()),
        file_uri(&workspace.join("app/inner-lib/library.pkl"))
    );
    assert_eq!(
        outer_target.text(state.database()).as_ref(),
        "owner = \"outer\"\n"
    );
    assert_eq!(
        inner_target.text(state.database()).as_ref(),
        "owner = \"inner\"\n"
    );
}

#[test]
fn missing_or_unsupported_import_targets_are_unresolved() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = Workspace::new();
    for uri in [
        "./missing.pkl",
        "modulepath:/library.pkl",
        "ftp://example.com/library.pkl",
    ] {
        let (source, tree) = source(
            &mut state,
            &temp.path().join("main.pkl"),
            &format!("import \"{uri}\"\n"),
        );
        assert!(
            matches!(
                resolve_import(state.database(), source, import_node(&tree), None),
                ImportResolution::Unresolved { .. }
            ),
            "unsupported or unresolved import: {uri}"
        );
    }
}

#[test]
fn glob_imports_resolve_and_non_import_nodes_are_unresolved() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("library.pkl"), "answer = 42\n").unwrap();
    let mut state = Workspace::new();
    let (source, tree) = source(
        &mut state,
        &temp.path().join("main.pkl"),
        "import* \"./library.pkl\"\nanswer = 1\n",
    );
    let glob = tree
        .child(tree.root(), |kind| matches!(kind, NodeKind::ImportGlob))
        .expect("fixture must contain a glob import");
    let property = tree
        .child(tree.root(), |kind| matches!(kind, NodeKind::ClassProperty))
        .unwrap();

    assert!(
        matches!(resolve_import(state.database(), source, glob, None), ImportResolution::Glob(targets) if targets.len() == 1)
    );
    assert!(matches!(
        resolve_import(state.database(), source, property, None),
        ImportResolution::Unresolved { .. }
    ));
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
    support::preload(state);
    state.document(url.as_str()).unwrap()
}

#[test]
fn semantic_context_keeps_shared_archive_imports_rooted_at_each_project() {
    use pkl_db::Db;
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    for name in ["outer", "inner"] {
        fs::create_dir_all(temp.path().join(name).join("lib")).unwrap();
    }
    fs::write(
        temp.path().join("outer/lib/library.pkl"),
        "value: Int = 1\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("inner/lib/library.pkl"),
        "value: String = \"inner\"\n",
    )
    .unwrap();
    let archive = temp.path().join("shared.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    writer
        .start_file("bridge.pkl", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer
        .write_all(b"import \"modulepath:/library.pkl\" as library\nvalue = library.value\n")
        .unwrap();
    writer.finish().unwrap();
    let remote = Uri::parse(format!("jar:{}!/bridge.pkl", file_uri(&archive))).unwrap();
    let mut state = Workspace::new();
    load_projects(
        &mut state,
        temp.path(),
        &[
            ("outer/PklProject", &["lib"]),
            ("inner/PklProject", &["lib"]),
        ],
    );
    state.apply_settings(pkl_db::settings::WorkspaceSettings {
        modulepath: vec![archive],
        ..Default::default()
    });
    let shared = state.database().file(&remote).unwrap();
    support::preload(&mut state);
    // A contextless warmup must not freeze the remote's project routing.
    {
        let view = state.snapshot();
        let ctx = pkl_types::AnalysisContext::for_root(&view.db, shared);
        assert!(
            ctx.import(pkl_types::NodeRef::new(
                shared,
                import_node(ctx.tree(shared).unwrap())
            ))
            .unwrap()
            .single()
            .is_none()
        );
    }
    let text = "import \"modulepath:/bridge.pkl\" as bridge\nanswer = bridge.value\n";
    let (outer, _) = source(&mut state, &temp.path().join("outer/main.pkl"), text);
    let (inner, _) = source(&mut state, &temp.path().join("inner/main.pkl"), text);
    let view = state.snapshot();
    for (root, expected) in [(outer, "Int"), (inner, "String"), (outer, "Int")] {
        let ctx = pkl_types::AnalysisContext::for_root(&view.db, root);
        let tree = ctx.tree(root).unwrap();
        let property = tree
            .child(tree.root(), |kind| matches!(kind, NodeKind::ClassProperty))
            .unwrap();
        assert_eq!(
            pkl_types::resolver::definition_type(
                ctx.db,
                pkl_types::NodeRef::new(root, property),
                ctx.root
            )
            .render(ctx.db),
            expected
        );
        let imported = ctx
            .import(pkl_types::NodeRef::new(root, import_node(tree)))
            .unwrap()
            .single()
            .unwrap();
        assert_eq!(imported.file, shared);
        assert!(pkl_db::project_for_file(&view.db, shared).is_none());
    }
}
