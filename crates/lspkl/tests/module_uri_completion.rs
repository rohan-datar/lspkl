use std::fs;
use std::io::Write;
use std::path::Path;

use fluent_uri::Uri;
use lsp_features::module_uri_completion::{completion, resolve};
use lsp_types::{
    CompletionItem, CompletionResponse, CompletionTextEdit, DidOpenTextDocumentParams,
    Documentation, Position, TextDocumentItem,
};
use lspkl::{Snapshot, Workspace};
use pkl_db::Db;
use pkl_syntax::TextSize;

fn uri(path: &Path) -> Uri<String> {
    pkl_vfs::path_to_uri(path).unwrap()
}
fn open_source(
    state: &mut Workspace,
    source_uri: Uri<String>,
    source: String,
    version: i32,
) -> Snapshot {
    state
        .apply_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: source_uri.as_str().parse().unwrap(),
                language_id: "pkl".into(),
                version,
                text: source,
            },
        })
        .unwrap();
    state.snapshot()
}
fn candidates(
    state: &mut Workspace,
    source_uri: Uri<String>,
    marked: &str,
) -> Option<Vec<CompletionItem>> {
    let cursor = marked.find('|').unwrap();
    let source = marked.replacen('|', "", 1);
    let workspace_snapshot = open_source(state, source_uri.clone(), source, 1);
    let file = workspace_snapshot.document(source_uri.as_str()).unwrap();
    match pkl_db::with_module_resolver(&workspace_snapshot.db, |resolver| {
        completion(
            &workspace_snapshot.db,
            file,
            TextSize::try_from(cursor).unwrap(),
            workspace_snapshot.position_encoding,
            resolver,
        )
    })? {
        CompletionResponse::Array(items) => Some(items),
        _ => panic!("expected deterministic array"),
    }
}
fn edit(item: &CompletionItem) -> &lsp_types::TextEdit {
    match item.text_edit.as_ref().unwrap() {
        CompletionTextEdit::Edit(edit) => edit,
        _ => panic!("expected content replacement"),
    }
}
fn docs(item: CompletionItem) -> String {
    match item.documentation.unwrap() {
        Documentation::MarkupContent(content) => content.value,
        Documentation::String(value) => value,
    }
}
fn archive(path: &Path, entries: &[(&str, &str)]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, source) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(source.as_bytes()).unwrap();
    }
    writer.finish().unwrap();
}
fn package(
    cache: &Path,
    name: &str,
    version: &str,
    dependencies: serde_json::Value,
    entries: &[(&str, &str)],
) -> String {
    let value = format!("package://example.test/{name}@{version}");
    let package = pkl_project::dto::PackageUri::create(&value).unwrap();
    let metadata = cache.join(&package.relative_metadata_files()[0]);
    fs::create_dir_all(metadata.parent().unwrap()).unwrap();
    fs::write(
        metadata,
        serde_json::to_vec(&serde_json::json!({
            "name": name, "version": version, "packageUri": value,
            "packageZipUrl": "https://example.test/package.zip", "dependencies": dependencies
        }))
        .unwrap(),
    )
    .unwrap();
    archive(&cache.join(&package.relative_zip_files()[0]), entries);
    value
}

#[test]
fn all_uri_owner_forms_and_embedded_modules_have_real_lazy_docs() {
    let mut state = Workspace::new();
    let temp = tempfile::tempdir().unwrap();
    for source in [
        "import \"pkl:|\"",
        "amends \"pkl:|\"",
        "extends \"pkl:|\"",
        "x = import(\"pkl:|\")",
    ] {
        let items = candidates(&mut state, uri(&temp.path().join("main.pkl")), source).unwrap();
        assert_eq!(items.len(), pkl_vfs::stdlib::MODULE_NAMES.len());
        let item = items
            .into_iter()
            .find(|item| item.label == "pkl:math")
            .unwrap();
        assert!(item.documentation.is_none());
        assert_eq!(item.data.as_ref().unwrap()["targetUri"], "pkl:math");
        let documentation = docs(resolve(&state.snapshot().db, item));
        assert!(documentation.contains("module pkl.math"));
        assert!(documentation.contains("mathematical"));
    }
    assert!(
        candidates(
            &mut state,
            uri(&temp.path().join("main.pkl")),
            "x = \"pkl:|\""
        )
        .is_none()
    );
}

#[test]
fn schemes_and_https_do_not_invent_children() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = Workspace::new();
    let source = uri(&temp.path().join("main.pkl"));
    let labels =
        |items: Vec<CompletionItem>| items.into_iter().map(|i| i.label).collect::<Vec<_>>();
    let items = candidates(&mut state, source.clone(), "import \"|\"").unwrap();
    for scheme in ["pkl:", "https://", "package://", "file:///", "modulepath:/"] {
        assert!(labels(items.clone()).contains(&scheme.to_owned()));
    }
    assert_eq!(
        labels(candidates(&mut state, source.clone(), "import* \"|\"").unwrap()),
        ["file:///", "package://"]
    );
    assert!(
        candidates(
            &mut state,
            source,
            "x = import*(\"https://example.test/|\")"
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn local_names_directories_utf16_raw_delimiters_and_lazy_target_docs() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("empty space")).unwrap();
    fs::write(
        temp.path().join("a % file.pkl"),
        "/// Unique target documentation.\nmodule fixture.target\nx = 1\n",
    )
    .unwrap();
    fs::write(temp.path().join(".hidden.pkl"), "x=1").unwrap();
    fs::write(temp.path().join("no.txt"), "no").unwrap();
    let mut state = Workspace::new();
    let items = candidates(
        &mut state,
        uri(&temp.path().join("main.pkl")),
        "x = \"😀\"\ny = import(#\"a |suffix\"#)",
    )
    .unwrap();
    let item = items
        .iter()
        .find(|item| item.label == "a % file.pkl")
        .unwrap();
    assert_eq!(edit(item).new_text, "a%20%25%20file.pkl");
    assert_eq!(edit(item).range.start, Position::new(1, 13));
    assert_eq!(edit(item).range.end, Position::new(1, 21));
    assert!(
        docs(resolve(&state.snapshot().db, item.clone())).contains("Unique target documentation")
    );
    let items = candidates(
        &mut state,
        uri(&temp.path().join("main.pkl")),
        "y = \"😀\" + import(\"|\")",
    )
    .unwrap();
    let item = items
        .iter()
        .find(|item| item.label == "empty space/")
        .unwrap();
    assert_eq!(edit(item).new_text, "empty%20space/");
    assert_eq!(edit(item).range.start, Position::new(0, 19));
    assert!(
        !items
            .iter()
            .any(|item| item.label == ".hidden.pkl" || item.label == "no.txt")
    );
}

#[test]
fn glob_completions_resolve_literal_metacharacters() {
    let temp = tempfile::tempdir().unwrap();
    let name = "literal[*{.pkl";
    let archive_path = temp.path().join("modules.zip");
    archive(
        &archive_path,
        &[
            (name, "module glob.target\nx=1"),
            ("literalOther.pkl", "x=2"),
        ],
    );
    let mut state = Workspace::new();
    let root = Uri::parse(format!("jar:{}!/", uri(&archive_path))).unwrap();
    let loader = pkl_vfs::StandardSourceLoader::default();
    let source_uri = pkl_vfs::SourceLoader::resolve_uri(&loader, &root, "main.pkl").unwrap();
    let target_uri = pkl_vfs::SourceLoader::resolve_uri(&loader, &root, name).unwrap();
    for marked in ["import* \"lit|\"", "x = import*(#\"lit|\"#)"] {
        let items = candidates(&mut state, source_uri.clone(), marked).unwrap();
        let item = items.iter().find(|item| item.label == name).unwrap();
        let replaced = marked.replacen("lit|", &edit(item).new_text, 1);
        let snapshot = open_source(&mut state, source_uri.clone(), replaced, 2);
        let file = snapshot.document(source_uri.as_str()).unwrap();
        let tree = pkl_db::parse(&snapshot.db, file).as_ref().unwrap();
        let mut pending = vec![tree.root()];
        let mut owners = Vec::new();
        while let Some(node) = pending.pop() {
            if tree.kind(node).is_module_uri_owner() {
                owners.push(node);
            }
            pending.extend_from_slice(tree.children(node));
        }
        assert_eq!(owners.len(), 1);
        let resolution = pkl_db::resolve_import(&snapshot.db, file, owners[0], None);
        match resolution {
            pkl_db::ImportResolution::Glob(targets) => {
                let expected = snapshot.db.file(&target_uri).unwrap();
                assert_eq!(targets, vec![expected]);
            }
            _ => panic!("glob expected"),
        }
    }
}

// APFS rejects invalid UTF-8 names before the completion code can run.
#[cfg(target_os = "linux")]
#[test]
fn filesystem_completion_keeps_sources_under_non_utf8_parent_paths() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join(OsStr::from_bytes(b"directory-\xff"));
    fs::create_dir(&parent).unwrap();
    let library = parent.join("library.pkl");
    fs::write(
        &library,
        "/// Native directory documentation.\nmodule library\nx = 1\n",
    )
    .unwrap();
    let mut state = Workspace::new();
    let items = candidates(&mut state, uri(&parent.join("main.pkl")), "import \"lib|\"").unwrap();
    let item = items
        .into_iter()
        .find(|item| item.label == "library.pkl")
        .expect("valid UTF-8 filenames remain visible under native non-UTF-8 parents");
    assert_eq!(
        item.data.as_ref().unwrap()["targetUri"],
        uri(&library).as_str()
    );
    assert!(docs(resolve(&state.snapshot().db, item)).contains("Native directory documentation."));
}

#[test]
fn modulepath_directories_zip_jar_precedence_and_docs_use_physical_target() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::write(
        root.join("same.pkl"),
        "/// Directory wins.\nmodule first\nx=1",
    )
    .unwrap();
    let zip = temp.path().join("modules.zip");
    archive(
        &zip,
        &[
            ("same.pkl", "module second\nx=2"),
            ("nested/member.pkl", "module zipped\nx=3"),
        ],
    );
    let jar = temp.path().join("modules.jar");
    archive(&jar, &[("jar.pkl", "module jarred\nx=4")]);
    let mut state = Workspace::new();
    state.apply_settings(pkl_db::settings::WorkspaceSettings {
        modulepath: vec![root.clone(), zip, jar],
        ..Default::default()
    });
    let items = candidates(
        &mut state,
        uri(&temp.path().join("main.pkl")),
        "import \"modulepath:/|\"",
    )
    .unwrap();
    assert_eq!(
        items.iter().filter(|item| item.label == "same.pkl").count(),
        1
    );
    assert!(items.iter().any(|item| item.label == "nested/"));
    assert!(items.iter().any(|item| item.label == "jar.pkl"));
    let same = items.iter().find(|item| item.label == "same.pkl").unwrap();
    assert_eq!(
        same.data.as_ref().unwrap()["targetUri"],
        uri(&root.join("same.pkl")).as_str()
    );
    assert!(docs(resolve(&state.snapshot().db, same.clone())).contains("Directory wins"));
    let items = candidates(
        &mut state,
        uri(&temp.path().join("main.pkl")),
        "import \"modulepath:/nested/|\"",
    )
    .unwrap();
    assert!(docs(resolve(&state.snapshot().db, items[0].clone())).contains("module zipped"));
}

#[test]
fn cached_package_bases_versions_members_and_transitive_dependency_names() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let b = package(
        &cache,
        "b",
        "1.0.0",
        serde_json::json!({}),
        &[(
            "docs member.pkl",
            "/// Transitive docs.\nmodule b.member\nx=1",
        )],
    );
    let a = package(
        &cache,
        "a",
        "1.0.0",
        serde_json::json!({"transitive": {"uri": b}}),
        &[("source.pkl", "x=1"), ("nested/other.pkl", "x=2")],
    );
    package(
        &cache,
        "a",
        "2.0.0",
        serde_json::json!({}),
        &[("source.pkl", "x=2")],
    );
    let mut state = Workspace::new();
    state.apply_settings(pkl_db::settings::WorkspaceSettings {
        cache_dir: Some(cache.clone()),
        ..Default::default()
    });
    let source_uri = uri(&temp.path().join("main.pkl"));
    let bases = candidates(
        &mut state,
        source_uri.clone(),
        "import \"package://example.test/|\"",
    )
    .unwrap();
    assert_eq!(
        bases
            .iter()
            .map(|item| edit(item).new_text.as_str())
            .collect::<Vec<_>>(),
        ["package://example.test/a@", "package://example.test/b@"]
    );
    let versions = candidates(
        &mut state,
        source_uri.clone(),
        "import \"package://example.test/a@|\"",
    )
    .unwrap();
    assert_eq!(
        versions
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        ["1.0.0", "2.0.0"]
    );
    assert_eq!(
        edit(&versions[1]).new_text,
        "package://example.test/a@2.0.0#/"
    );
    let members = candidates(&mut state, source_uri, &format!("import \"{a}#/|\"")).unwrap();
    assert!(members.iter().any(|item| item.label == "nested/"));
    let package_uri = pkl_project::dto::PackageUri::create(&a).unwrap();
    let jar_uri = format!(
        "jar:{}!/source.pkl",
        uri(&cache.join(&package_uri.relative_zip_files()[0]))
    );
    let source = Uri::parse(jar_uri).unwrap();
    let names = candidates(&mut state, source.clone(), "import \"@|\"").unwrap();
    assert_eq!(names[0].label, "@transitive");
    let members = candidates(&mut state, source, "import \"@transitive/|\"").unwrap();
    assert_eq!(edit(&members[0]).new_text, "@transitive/docs%20member.pkl");
    assert!(docs(resolve(&state.snapshot().db, members[0].clone())).contains("Transitive docs"));
}

#[test]
fn named_local_and_remote_project_dependencies_complete_members() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let remote = package(
        &cache,
        "remote",
        "1.0.0",
        serde_json::json!({}),
        &[("remote.pkl", "module remote.member\nx=1")],
    );
    let local_uri = "package://example.test/local@1.0.0";
    fs::create_dir(temp.path().join("local")).unwrap();
    fs::write(
        temp.path().join("local/member.pkl"),
        "/// Local dependency docs.\nmodule local.member\nx=1",
    )
    .unwrap();
    fs::write(temp.path().join("PklProject"), "").unwrap();
    let state_file = temp.path().join("workspace.json");
    fs::write(&state_file, serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 1, "pklProjects": [{
            "projectFile": "PklProject", "packageUri": null,
            "declaredDependencies": {"local": local_uri, "remote": remote},
            "resolvedDependencies": {
                "package://example.test/local@1": {"type": "local", "uri": local_uri, "path": "local"},
                "package://example.test/remote@1": {"type": "remote", "uri": remote}
            }, "evaluatorSettings": null
        }]
    })).unwrap()).unwrap();
    let mut state = Workspace::new();
    let snapshot = state.snapshot();
    {
        let mut projects = snapshot.project_manager.lock();
        projects.initialize(vec![temp.path().to_owned()]);
        projects.load_state(&state_file).unwrap();
    }
    drop(snapshot);
    state.refresh_projects();
    state.apply_settings(pkl_db::settings::WorkspaceSettings {
        cache_dir: Some(cache),
        ..Default::default()
    });
    let source = uri(&temp.path().join("main.pkl"));
    let names = candidates(&mut state, source.clone(), "import \"@|\"").unwrap();
    assert_eq!(
        names
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        ["@local", "@remote"]
    );
    let items = candidates(&mut state, source.clone(), "import \"@local/|\"").unwrap();
    assert_eq!(
        items[0].data.as_ref().unwrap()["targetUri"],
        uri(&temp.path().join("local/member.pkl")).as_str()
    );
    assert!(
        docs(resolve(&state.snapshot().db, items[0].clone())).contains("Local dependency docs")
    );
    let items = candidates(&mut state, source, "import \"@remote/|\"").unwrap();
    assert!(docs(resolve(&state.snapshot().db, items[0].clone())).contains("module remote.member"));
}

#[test]
fn absolute_file_upward_paths_and_raw_delimiters_replace_only_contents() {
    let temp = tempfile::tempdir().unwrap();
    let nested = temp.path().join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(temp.path().join("parent.pkl"), "module parent\nx=1").unwrap();
    let mut state = Workspace::new();
    let source = uri(&nested.join("main.pkl"));
    let items = candidates(&mut state, source.clone(), "import \".../par|\"").unwrap();
    assert_eq!(edit(&items[0]).new_text, ".../parent.pkl");
    let absolute = format!("import \"{}|\"", uri(temp.path()));
    let absolute = absolute.replace("|", "/par|");
    let items = candidates(&mut state, source.clone(), &absolute).unwrap();
    assert!(edit(&items[0]).new_text.ends_with("/parent.pkl"));
    let marked = "x = import(#\"pkl:ma|\"#)";
    let items = candidates(&mut state, source, marked).unwrap();
    assert_eq!(edit(&items[0]).new_text, "pkl:math");
    assert_eq!(edit(&items[0]).range.start, Position::new(0, 13));
}
