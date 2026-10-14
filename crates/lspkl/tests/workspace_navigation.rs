//! Consumer-visible project/import behavior through the real backend handlers.

mod support;

use support::initialize_workspace;

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use lsp_types::*;
use lspkl::{Workspace, handlers};
use pkl_project::dto::{PackageMetadata, PackageUri, RemoteDependency};
use serde_json::{Value, json};

fn uri(path: &Path) -> Uri {
    url::Url::from_file_path(path)
        .unwrap()
        .as_str()
        .parse()
        .unwrap()
}

fn initialize(backend: &mut Workspace, folders: &[Uri], settings: Value) {
    initialize_workspace(backend, serde_json::from_value(json!({
        "capabilities": {},
        "workspaceFolders": folders.iter().map(|uri| json!({"uri": uri, "name": "workspace"})).collect::<Vec<_>>(),
        "initializationOptions": {"pkl": settings},
    })).unwrap()).unwrap();
}

fn open(backend: &mut Workspace, uri: &Uri, source: &str) {
    handlers::lifecycle::did_open(
        backend,
        DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "pkl".into(),
                version: 1,
                text: source.into(),
            },
        },
    )
    .unwrap();
    support::preload(backend);
}

fn position(source: &str, token: &str) -> Position {
    let offset = source.rfind(token).unwrap();
    let prefix = &source[..offset];
    Position::new(
        prefix.bytes().filter(|&byte| byte == b'\n').count() as u32,
        prefix.rsplit('\n').next().unwrap().encode_utf16().count() as u32,
    )
}

fn definition(backend: &mut Workspace, uri: &Uri, position: Position) -> GotoDefinitionResponse {
    support::preload(backend);
    handlers::documents::definition(
        &backend.snapshot(),
        GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        },
    )
    .unwrap()
    .expect("definition must resolve")
}

fn scalar(response: GotoDefinitionResponse) -> Location {
    let GotoDefinitionResponse::Scalar(location) = response else {
        panic!("expected a scalar location")
    };
    location
}

#[test]
fn imported_member_navigation_and_hover_use_the_unsaved_target_tree() {
    let temp = tempfile::tempdir().unwrap();
    let library = uri(&temp.path().join("library.pkl"));
    fs::write(temp.path().join("library.pkl"), "value: Int = 1\n").unwrap();
    let main = uri(&temp.path().join("main.pkl"));
    let mut backend = Workspace::new();
    open(
        &mut backend,
        &library,
        "// unsaved target\n\nvalue: String = \"edited\"\n",
    );
    let source = "import \"./library.pkl\" as library\nanswer = library.value\n";
    open(&mut backend, &main, source);
    let cursor = position(source, "value");
    let location = scalar(definition(&mut backend, &main, cursor));
    assert_eq!(location.uri, library);
    assert_eq!(location.range.start, Position::new(2, 0));
    let hover = handlers::documents::hover(
        &backend.snapshot(),
        HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: main },
                position: cursor,
            },
            work_done_progress_params: Default::default(),
        },
    )
    .unwrap()
    .unwrap();
    let HoverContents::Markup(contents) = hover.contents else {
        panic!("expected Markdown hover")
    };
    assert!(
        contents.value.contains("value: String"),
        "{}",
        contents.value
    );
}

#[test]
fn inherited_members_import_expressions_and_glob_uri_navigation_share_the_graph() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("libs")).unwrap();
    fs::write(temp.path().join("parent.pkl"), "inherited: Int = 7\n").unwrap();
    fs::write(temp.path().join("libs/a.pkl"), "value: Int = 1\n").unwrap();
    fs::write(temp.path().join("libs/b.pkl"), "value: Int = 2\n").unwrap();
    let source = "amends \"./parent.pkl\"\nimport* \"./libs/*.pkl\" as libraries\nanswer = inherited\nlibrary = import(\"./libs/a.pkl\")\nother = library.value\n";
    let main = uri(&temp.path().join("main.pkl"));
    let mut backend = Workspace::new();
    open(&mut backend, &main, source);
    let inherited = scalar(definition(
        &mut backend,
        &main,
        position(source, "inherited"),
    ));
    assert_eq!(inherited.uri, uri(&temp.path().join("parent.pkl")));
    let expression = scalar(definition(&mut backend, &main, position(source, "value")));
    assert_eq!(expression.uri, uri(&temp.path().join("libs/a.pkl")));
    let response = definition(&mut backend, &main, position(source, "./libs/*.pkl"));
    let GotoDefinitionResponse::Array(locations) = response else {
        panic!("glob URI must return all matched modules")
    };
    let mut targets: Vec<_> = locations.into_iter().map(|location| location.uri).collect();
    targets.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    assert_eq!(
        targets,
        [
            uri(&temp.path().join("libs/a.pkl")),
            uri(&temp.path().join("libs/b.pkl"))
        ]
    );
}

#[test]
fn declared_global_paths_rebind_on_folder_changes_and_configuration_updates() {
    let temp = tempfile::tempdir().unwrap();
    let one = temp.path().join("one");
    let two = temp.path().join("two");
    for folder in [one.join("lib"), two.join("lib"), two.join("override")] {
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("library.pkl"), "value = 1\n").unwrap();
    }
    let mut backend = Workspace::new();
    initialize(&mut backend, &[uri(&one)], json!({"modulepath": ["lib"]}));
    let main = uri(&temp.path().join("main.pkl"));
    let source = "import \"modulepath:/library.pkl\" as library\nanswer = library.value\n";
    open(&mut backend, &main, source);
    let cursor = position(source, "value");
    assert_eq!(
        scalar(definition(&mut backend, &main, cursor)).uri,
        uri(&one.join("lib/library.pkl"))
    );
    handlers::lifecycle::did_change_workspace_folders(
        &mut backend,
        DidChangeWorkspaceFoldersParams {
            event: WorkspaceFoldersChangeEvent {
                removed: vec![WorkspaceFolder {
                    uri: uri(&one),
                    name: "one".into(),
                }],
                added: vec![WorkspaceFolder {
                    uri: uri(&two),
                    name: "two".into(),
                }],
            },
        },
    );
    assert_eq!(
        scalar(definition(&mut backend, &main, cursor)).uri,
        uri(&two.join("lib/library.pkl"))
    );
    handlers::lifecycle::did_change_configuration(
        &mut backend,
        DidChangeConfigurationParams {
            settings: json!({"pkl": {"modulepath": ["override"]}}),
        },
    )
    .unwrap();
    assert_eq!(
        scalar(definition(&mut backend, &main, cursor)).uri,
        uri(&two.join("override/library.pkl"))
    );
}

#[test]
fn initialization_restores_project_modulepaths_without_invoking_the_cli() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join(".lspkl")).unwrap();
    fs::create_dir(temp.path().join("lib")).unwrap();
    fs::write(temp.path().join("lib/library.pkl"), "value = 1\n").unwrap();
    fs::write(temp.path().join("PklProject"), "amends \"pkl:Project\"\n").unwrap();
    fs::write(
        temp.path().join(".lspkl/projects.json"),
        serde_json::to_vec(&json!({
            "schemaVersion": 1,
            "pklProjects": [{"projectFile": "PklProject", "packageUri": null,
                "declaredDependencies": {}, "resolvedDependencies": null,
                "evaluatorSettings": {"modulePath": ["lib"]}}],
        }))
        .unwrap(),
    )
    .unwrap();
    let mut backend = Workspace::new();
    initialize(
        &mut backend,
        &[uri(temp.path())],
        json!({"cli": {"path": "/no/such/pkl"}}),
    );
    let source = "import \"modulepath:/library.pkl\" as library\nanswer = library.value\n";
    let main = uri(&temp.path().join("main.pkl"));
    open(&mut backend, &main, source);
    assert_eq!(
        scalar(definition(&mut backend, &main, position(source, "value"))).uri,
        uri(&temp.path().join("lib/library.pkl"))
    );
    fs::write(
        temp.path().join("PklProject"),
        "amends \"pkl:Project\"\n// changed on disk\n",
    )
    .unwrap();
    {
        let work = backend.prepare_watched_files(DidChangeWatchedFilesParams {
            changes: vec![FileEvent {
                uri: uri(&temp.path().join("PklProject")),
                typ: FileChangeType::CHANGED,
            }],
        });
        backend.apply_reload(work.load());
    };
    let result = handlers::documents::definition(
        &backend.snapshot(),
        GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: main },
                position: position(source, "value"),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        },
    )
    .unwrap();
    assert!(
        result.is_none(),
        "changed PklProject must invalidate stale cached modulepaths: {result:?}"
    );
    assert_eq!(backend.project_manager().projects().count(), 0);
}

fn package(
    cache: &Path,
    name: &str,
    dependencies: BTreeMap<String, RemoteDependency>,
    source: &str,
) -> PackageUri {
    let package = PackageUri::create(&format!("package://example.test/{name}@1.0.0")).unwrap();
    let metadata = PackageMetadata {
        name: name.into(),
        version: "1.0.0".into(),
        package_zip_url: "https://example.test/package.zip".into(),
        package_uri: package.clone(),
        dependencies,
    };
    let path = cache.join(&package.relative_metadata_files()[0]);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let mut zip = zip::ZipWriter::new(
        fs::File::create(cache.join(&package.relative_zip_files()[0])).unwrap(),
    );
    zip.start_file("lib.pkl", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(source.as_bytes()).unwrap();
    zip.finish().unwrap();
    package
}

#[test]
fn cached_transitive_packages_and_stdlib_return_editor_openable_definitions() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let b = package(&cache, "b", BTreeMap::new(), "value: Int = 42\n");
    let a = package(
        &cache,
        "a",
        BTreeMap::from([(
            "b".into(),
            RemoteDependency {
                uri: b,
                checksums: None,
            },
        )]),
        "import \"@b/lib.pkl\" as b\nvalue = b.value\n",
    );
    let source = format!(
        "import \"{}#/lib.pkl\" as library\nanswer = library.value\ntyped: String = \"hello\"\n",
        a.to_display_string()
    );
    let mut backend = Workspace::new();
    initialize(
        &mut backend,
        &[uri(temp.path())],
        json!({"cacheDir": cache}),
    );
    let main = uri(&temp.path().join("main.pkl"));
    open(&mut backend, &main, &source);
    let imported = scalar(definition(&mut backend, &main, position(&source, "value")));
    assert!(imported.uri.as_str().starts_with("lspkl://jar/"));
    assert_eq!(imported.range.start, Position::new(1, 0));
    let hover = handlers::documents::hover(
        &backend.snapshot(),
        HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: main.clone() },
                position: position(&source, "value"),
            },
            work_done_progress_params: Default::default(),
        },
    )
    .unwrap()
    .unwrap();
    let HoverContents::Markup(contents) = hover.contents else {
        panic!("expected Markdown hover")
    };
    assert!(contents.value.contains("value: Int"), "{}", contents.value);
    let stdlib = scalar(definition(&mut backend, &main, position(&source, "String")));
    assert_eq!(stdlib.uri.as_str(), "lspkl://stdlib/base.pkl");
}

#[test]
fn source_rpc_install_does_not_cancel_a_newer_watched_read() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.pkl");
    fs::write(&path, "value: Int = 1\n").unwrap();
    let mut workspace = lspkl::Workspace::new();
    let view = workspace.snapshot();
    let request = lspkl::handlers::documents::prepare_file_contents(
        &view,
        TextDocumentIdentifier { uri: uri(&path) },
    )
    .unwrap();
    drop(view);
    let old = lspkl::handlers::documents::file_contents(request)
        .unwrap()
        .source;
    fs::write(&path, "value: String = \"new\"\n").unwrap();
    let watched = workspace.prepare_watched_files(DidChangeWatchedFilesParams {
        changes: vec![FileEvent {
            uri: uri(&path),
            typ: FileChangeType::CHANGED,
        }],
    });
    workspace.apply_loaded_source(old).unwrap();
    workspace.apply_reload(watched.load());
    let view = workspace.snapshot();
    let request = lspkl::handlers::documents::prepare_file_contents(
        &view,
        TextDocumentIdentifier { uri: uri(&path) },
    )
    .unwrap();
    drop(view);
    assert_eq!(
        lspkl::handlers::documents::file_contents(request)
            .unwrap()
            .source
            .contents(),
        "value: String = \"new\"\n"
    );
}
