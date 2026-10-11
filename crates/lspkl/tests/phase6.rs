//! Consumer-visible project/import behavior through the real backend handlers.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use lspkl::PklBackend;
use pkl_project::dto::{PackageMetadata, PackageUri, RemoteDependency};
use serde_json::{Value, json};
use tower_lsp_server::LanguageServer;
use tower_lsp_server::lsp_types::*;

fn uri(path: &Path) -> Uri {
    url::Url::from_file_path(path)
        .unwrap()
        .as_str()
        .parse()
        .unwrap()
}

async fn initialize(backend: &PklBackend, folders: &[Uri], settings: Value) {
    backend.initialize(serde_json::from_value(json!({
        "capabilities": {},
        "workspaceFolders": folders.iter().map(|uri| json!({"uri": uri, "name": "workspace"})).collect::<Vec<_>>(),
        "initializationOptions": {"pkl": settings},
    })).unwrap()).await.unwrap();
}

async fn open(backend: &PklBackend, uri: &Uri, source: &str) {
    backend
        .did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "pkl".into(),
                version: 1,
                text: source.into(),
            },
        })
        .await;
}

fn position(source: &str, token: &str) -> Position {
    let offset = source.rfind(token).unwrap();
    let prefix = &source[..offset];
    Position::new(
        prefix.bytes().filter(|&byte| byte == b'\n').count() as u32,
        prefix.rsplit('\n').next().unwrap().encode_utf16().count() as u32,
    )
}

async fn definition(backend: &PklBackend, uri: &Uri, position: Position) -> GotoDefinitionResponse {
    backend
        .goto_definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .await
        .unwrap()
        .expect("definition must resolve")
}

fn scalar(response: GotoDefinitionResponse) -> Location {
    let GotoDefinitionResponse::Scalar(location) = response else {
        panic!("expected a scalar location")
    };
    location
}

#[tokio::test]
async fn imported_member_navigation_and_hover_use_the_unsaved_target_tree() {
    let temp = tempfile::tempdir().unwrap();
    let library = uri(&temp.path().join("library.pkl"));
    fs::write(temp.path().join("library.pkl"), "value: Int = 1\n").unwrap();
    let main = uri(&temp.path().join("main.pkl"));
    let backend = PklBackend::for_testing();
    open(
        &backend,
        &library,
        "// unsaved target\n\nvalue: String = \"edited\"\n",
    )
    .await;
    let source = "import \"./library.pkl\" as library\nanswer = library.value\n";
    open(&backend, &main, source).await;
    let cursor = position(source, "value");
    let location = scalar(definition(&backend, &main, cursor).await);
    assert_eq!(location.uri, library);
    assert_eq!(location.range.start, Position::new(2, 0));
    let hover = backend
        .hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: main },
                position: cursor,
            },
            work_done_progress_params: Default::default(),
        })
        .await
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

#[tokio::test]
async fn inherited_members_import_expressions_and_glob_uri_navigation_share_the_graph() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("libs")).unwrap();
    fs::write(temp.path().join("parent.pkl"), "inherited: Int = 7\n").unwrap();
    fs::write(temp.path().join("libs/a.pkl"), "value: Int = 1\n").unwrap();
    fs::write(temp.path().join("libs/b.pkl"), "value: Int = 2\n").unwrap();
    let source = "amends \"./parent.pkl\"\nimport* \"./libs/*.pkl\" as libraries\nanswer = inherited\nlibrary = import(\"./libs/a.pkl\")\nother = library.value\n";
    let main = uri(&temp.path().join("main.pkl"));
    let backend = PklBackend::for_testing();
    open(&backend, &main, source).await;
    let inherited = scalar(definition(&backend, &main, position(source, "inherited")).await);
    assert_eq!(inherited.uri, uri(&temp.path().join("parent.pkl")));
    let expression = scalar(definition(&backend, &main, position(source, "value")).await);
    assert_eq!(expression.uri, uri(&temp.path().join("libs/a.pkl")));
    let response = definition(&backend, &main, position(source, "./libs/*.pkl")).await;
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

#[tokio::test]
async fn declared_global_paths_rebind_on_folder_changes_and_configuration_updates() {
    let temp = tempfile::tempdir().unwrap();
    let one = temp.path().join("one");
    let two = temp.path().join("two");
    for folder in [one.join("lib"), two.join("lib"), two.join("override")] {
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("library.pkl"), "value = 1\n").unwrap();
    }
    let backend = PklBackend::for_testing();
    initialize(&backend, &[uri(&one)], json!({"modulepath": ["lib"]})).await;
    let main = uri(&temp.path().join("main.pkl"));
    let source = "import \"modulepath:/library.pkl\" as library\nanswer = library.value\n";
    open(&backend, &main, source).await;
    let cursor = position(source, "value");
    assert_eq!(
        scalar(definition(&backend, &main, cursor).await).uri,
        uri(&one.join("lib/library.pkl"))
    );
    backend
        .did_change_workspace_folders(DidChangeWorkspaceFoldersParams {
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
        })
        .await;
    assert_eq!(
        scalar(definition(&backend, &main, cursor).await).uri,
        uri(&two.join("lib/library.pkl"))
    );
    backend
        .did_change_configuration(DidChangeConfigurationParams {
            settings: json!({"pkl": {"modulepath": ["override"]}}),
        })
        .await;
    assert_eq!(
        scalar(definition(&backend, &main, cursor).await).uri,
        uri(&two.join("override/library.pkl"))
    );
}

#[tokio::test]
async fn initialization_restores_project_modulepaths_without_invoking_the_cli() {
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
    let backend = PklBackend::for_testing();
    initialize(
        &backend,
        &[uri(temp.path())],
        json!({"cli": {"path": "/no/such/pkl"}}),
    )
    .await;
    let source = "import \"modulepath:/library.pkl\" as library\nanswer = library.value\n";
    let main = uri(&temp.path().join("main.pkl"));
    open(&backend, &main, source).await;
    assert_eq!(
        scalar(definition(&backend, &main, position(source, "value")).await).uri,
        uri(&temp.path().join("lib/library.pkl"))
    );
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

#[tokio::test]
async fn cached_transitive_packages_and_stdlib_return_editor_openable_definitions() {
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
    let backend = PklBackend::for_testing();
    initialize(&backend, &[uri(temp.path())], json!({"cacheDir": cache})).await;
    let main = uri(&temp.path().join("main.pkl"));
    open(&backend, &main, &source).await;
    let imported = scalar(definition(&backend, &main, position(&source, "value")).await);
    assert!(imported.uri.as_str().starts_with("lspkl://jar/"));
    assert_eq!(imported.range.start, Position::new(1, 0));
    let hover = backend
        .hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: main.clone() },
                position: position(&source, "value"),
            },
            work_done_progress_params: Default::default(),
        })
        .await
        .unwrap()
        .unwrap();
    let HoverContents::Markup(contents) = hover.contents else {
        panic!("expected Markdown hover")
    };
    assert!(contents.value.contains("value: Int"), "{}", contents.value);
    let stdlib = scalar(definition(&backend, &main, position(&source, "String")).await);
    assert_eq!(stdlib.uri.as_str(), "lspkl://stdlib/base.pkl");
}
