//! Navigation, hover, completion, and lifecycle semantics over the shipped binary.
#[path = "support/https.rs"]
mod https;
mod support;

use serde_json::{Value, json};
use std::{fs, io::Write, path::Path};
use support::LspClient;

fn uri(path: &Path) -> String {
    pkl_vfs::path_to_uri(path).unwrap().into_string()
}
fn position(source: &str, token: &str) -> Value {
    let offset = source.rfind(token).unwrap();
    let prefix = &source[..offset];
    json!({"line":prefix.bytes().filter(|b| *b == b'\n').count(),
        "character":prefix.rsplit('\n').next().unwrap().encode_utf16().count()})
}
fn range(line: u32, start: u32, end: u32) -> Value {
    json!({"start":{"line":line,"character":start},"end":{"line":line,"character":end}})
}
async fn start(workspace: &Path, settings: Value) -> LspClient {
    let mut client = LspClient::spawn(Path::new(env!("CARGO_BIN_EXE_lspkl")), workspace).await;
    let response = client
        .request(
            "initialize",
            json!({
                "processId":null,"rootUri":uri(workspace),
                "workspaceFolders":[{"uri":uri(workspace),"name":"fixture"}],
                "capabilities":{"workspace":{"configuration":true}},
                "initializationOptions":settings
            }),
        )
        .await;
    assert!(response.get("error").is_none(), "{response}");
    client.notify("initialized", json!({})).await;
    client
}
async fn open(client: &mut LspClient, uri: &str, source: &str) {
    client
        .notify(
            "textDocument/didOpen",
            json!({"textDocument":{
                "uri":uri,"languageId":"pkl","version":1,"text":source
            }}),
        )
        .await;
}
async fn diagnostics(client: &mut LspClient, uri: &str) -> Value {
    loop {
        let mut notification = client.notification("textDocument/publishDiagnostics").await;
        if notification["params"]["uri"] == uri {
            return notification["params"].take();
        }
    }
}
async fn at(client: &mut LspClient, method: &str, uri: &str, source: &str, token: &str) -> Value {
    let params = json!({"textDocument":{"uri":uri},"position":position(source,token)});
    let mut response = client.request(method, params.clone()).await;
    if response.get("error").is_some() {
        assert_eq!(response["error"]["code"], -32801, "{response}");
        response = client.request(method, params).await;
    }
    assert!(
        response.get("error").is_none(),
        "quiescent retry failed: {response}"
    );
    response["result"].take()
}

#[tokio::test]
async fn local_and_imported_definitions_have_target_ranges_and_foreign_hover_docs() {
    let temp = tempfile::tempdir().unwrap();
    let library = uri(&temp.path().join("library.pkl"));
    let main = uri(&temp.path().join("main.pkl"));
    fs::write(
        temp.path().join("library.pkl"),
        "/// Foreign property documentation.\nvalue: Int = 42\n",
    )
    .unwrap();
    let source =
        "import \"./library.pkl\" as library\nprop = 42\nmyProp = prop\nresult = library.value\n";
    let mut client = start(temp.path(), json!({})).await;
    open(&mut client, &main, source).await;
    assert_eq!(
        diagnostics(&mut client, &main).await["diagnostics"],
        json!([])
    );
    let local = at(
        &mut client,
        "textDocument/definition",
        &main,
        source,
        "prop",
    )
    .await;
    assert_eq!(local, json!({"uri":main,"range":range(1,0,9)}));
    let imported = at(
        &mut client,
        "textDocument/definition",
        &main,
        source,
        "value",
    )
    .await;
    assert_eq!(
        imported,
        json!({"uri":library,"range":{"start":{"line":0,"character":0},"end":{"line":1,"character":15}}})
    );
    let hover = at(&mut client, "textDocument/hover", &main, source, "value").await;
    assert_eq!(hover["contents"]["kind"], "markdown");
    let markdown = hover["contents"]["value"].as_str().unwrap();
    assert!(markdown.contains("value: Int"), "{markdown}");
    assert!(
        markdown.contains("Foreign property documentation."),
        "{markdown}"
    );
    client.shutdown().await;
}

#[tokio::test]
async fn invalid_initialize_can_be_retried_with_open_stdin() {
    let temp = tempfile::tempdir().unwrap();
    let mut client = LspClient::spawn(Path::new(env!("CARGO_BIN_EXE_lspkl")), temp.path()).await;
    let invalid = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.request("initialize", json!({})),
    )
    .await
    .expect("invalid initialization must receive a response");
    assert_eq!(invalid["error"]["code"], -32602, "{invalid}");
    let valid = client
        .request("initialize", json!({"capabilities":{}}))
        .await;
    assert!(valid.get("error").is_none(), "{valid}");
    client.notify("initialized", json!({})).await;
    client.shutdown().await;
}

#[tokio::test]
async fn rapid_changes_publish_diagnostics_for_the_final_text() {
    let temp = tempfile::tempdir().unwrap();
    let document = uri(&temp.path().join("burst.pkl"));
    let mut client = start(temp.path(), json!({})).await;
    open(&mut client, &document, "value: Int = 1\n").await;
    assert_eq!(
        diagnostics(&mut client, &document).await["diagnostics"],
        json!([])
    );

    const FINAL_VERSION: i32 = 129;
    for version in 2..=FINAL_VERSION {
        let text = if version == FINAL_VERSION {
            "value: Int = missing_final\n"
        } else if version % 2 == 0 {
            "value: Int = 1\n"
        } else {
            "value: Int = missing_intermediate\n"
        };
        client
            .notify(
                "textDocument/didChange",
                json!({"textDocument":{"uri":document,"version":version},
                    "contentChanges":[{"text":text}]}),
            )
            .await;
    }
    let final_publish = loop {
        let published = diagnostics(&mut client, &document).await;
        if published["version"] == FINAL_VERSION {
            break published;
        }
    };
    let errors = final_publish["diagnostics"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{final_publish}");
    assert_eq!(errors[0]["severity"], 1, "{final_publish}");
    assert_eq!(errors[0]["range"], range(0, 13, 26), "{final_publish}");
    assert!(
        errors[0]["message"]
            .as_str()
            .unwrap()
            .contains("missing_final"),
        "{final_publish}"
    );
    client.shutdown().await;
}

#[tokio::test]
async fn edits_refresh_dependent_diagnostics_and_close_restores_disk_source() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("library.pkl"), "value: Int = 1\n").unwrap();
    let library = uri(&temp.path().join("library.pkl"));
    let main = uri(&temp.path().join("main.pkl"));
    let source = "import \"./library.pkl\" as library\nanswer = library.value\n";
    let mut client = start(temp.path(), json!({})).await;
    open(&mut client, &library, "value: Int = 1\n").await;
    assert_eq!(
        diagnostics(&mut client, &library).await["diagnostics"],
        json!([])
    );
    open(&mut client, &main, source).await;
    assert_eq!(
        diagnostics(&mut client, &main).await["diagnostics"],
        json!([])
    );
    client
        .notify(
            "textDocument/didChange",
            json!({"textDocument":{"uri":library,"version":2},
        "contentChanges":[{"text":"other: String = \"unsaved\"\n"}]}),
        )
        .await;
    let changed = diagnostics(&mut client, &main).await;
    let errors = changed["diagnostics"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{changed}");
    assert_eq!(errors[0]["severity"], 1);
    assert_eq!(errors[0]["range"], range(1, 17, 22));
    assert_eq!(
        at(
            &mut client,
            "textDocument/definition",
            &main,
            source,
            "value"
        )
        .await,
        Value::Null
    );
    client
        .notify(
            "textDocument/didClose",
            json!({"textDocument":{"uri":library}}),
        )
        .await;
    assert_eq!(
        diagnostics(&mut client, &main).await["diagnostics"],
        json!([])
    );
    let restored = at(
        &mut client,
        "textDocument/definition",
        &main,
        source,
        "value",
    )
    .await;
    assert_eq!(restored, json!({"uri":library,"range":range(0,0,14)}));
    let hover = at(&mut client, "textDocument/hover", &main, source, "value").await;
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("value: Int")
    );
    client.shutdown().await;
}

#[tokio::test]
async fn uri_completions_resolve_real_local_and_embedded_documentation() {
    let temp = tempfile::Builder::new()
        .prefix("lspkl sources [λ] ")
        .tempdir()
        .unwrap();
    fs::write(
        temp.path().join("target.pkl"),
        "/// Target module documentation.\nmodule fixture.target\nvalue = 1\n",
    )
    .unwrap();
    fs::write(temp.path().join("ignore.txt"), "not a module").unwrap();
    let main = uri(&temp.path().join("main.pkl"));
    let mut client = start(temp.path(), json!({})).await;
    let source = "answer = import(\"tar\")\n";
    open(&mut client, &main, source).await;
    diagnostics(&mut client, &main).await;
    // Cursor is after the prefix rather than at its first character.
    let response = client
        .request(
            "textDocument/completion",
            json!({"textDocument":{"uri":main},"position":{"line":0,"character":20}}),
        )
        .await;
    assert!(response.get("error").is_none(), "{response}");
    let items = response["result"].as_array().unwrap();
    assert_eq!(
        items
            .iter()
            .map(|i| i["label"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["target.pkl"]
    );
    let item = &items[0];
    assert!(item.get("documentation").is_none());
    assert_eq!(
        item["textEdit"],
        json!({"range":range(0,17,20),"newText":"target.pkl"})
    );
    let resolved = client.request("completionItem/resolve", item.clone()).await;
    assert!(
        resolved["result"]["documentation"]["value"]
            .as_str()
            .unwrap()
            .contains("Target module documentation.")
    );
    client.notify("textDocument/didChange",json!({"textDocument":{"uri":main,"version":2},"contentChanges":[{"text":"amends \"pkl:ma\"\n"}]})).await;
    diagnostics(&mut client, &main).await;
    let response = client
        .request(
            "textDocument/completion",
            json!({"textDocument":{"uri":main},"position":{"line":0,"character":14}}),
        )
        .await;
    let items = response["result"].as_array().unwrap();
    assert_eq!(
        items
            .iter()
            .map(|i| i["label"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["pkl:math"]
    );
    assert!(items[0].get("documentation").is_none());
    let resolved = client
        .request("completionItem/resolve", items[0].clone())
        .await;
    let docs = resolved["result"]["documentation"]["value"]
        .as_str()
        .unwrap();
    assert!(
        docs.contains("module pkl.math") && docs.contains("mathematical"),
        "{docs}"
    );
    client.shutdown().await;
}

#[tokio::test]
async fn inherited_and_glob_definitions_resolve_distinct_modules() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("libs")).unwrap();
    fs::write(temp.path().join("parent.pkl"), "inherited: Int = 7\n").unwrap();
    fs::write(temp.path().join("libs/a.pkl"), "value = 1\n").unwrap();
    fs::write(temp.path().join("libs/b.pkl"), "value = 2\n").unwrap();
    let main = uri(&temp.path().join("main.pkl"));
    let source =
        "amends \"./parent.pkl\"\nimport* \"./libs/*.pkl\" as libraries\nanswer = inherited\n";
    let mut client = start(temp.path(), json!({})).await;
    open(&mut client, &main, source).await;
    diagnostics(&mut client, &main).await;
    assert_eq!(
        at(
            &mut client,
            "textDocument/definition",
            &main,
            source,
            "inherited"
        )
        .await,
        json!({"uri":uri(&temp.path().join("parent.pkl")),"range":range(0,0,18)})
    );
    let glob = at(
        &mut client,
        "textDocument/definition",
        &main,
        source,
        "./libs/*.pkl",
    )
    .await;
    let mut targets = glob
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["uri"].as_str().unwrap())
        .collect::<Vec<_>>();
    targets.sort_unstable();
    assert_eq!(
        targets,
        [
            uri(&temp.path().join("libs/a.pkl")),
            uri(&temp.path().join("libs/b.pkl"))
        ]
    );
    client.shutdown().await;
}

#[tokio::test]
async fn stdlib_navigation_source_rpc_and_missing_source_error() {
    let temp = tempfile::tempdir().unwrap();
    let main = uri(&temp.path().join("main.pkl"));
    let source = "name: String = \"Pkl\"\n";
    let mut client = start(temp.path(), json!({})).await;
    open(&mut client, &main, source).await;
    diagnostics(&mut client, &main).await;
    let definition = at(
        &mut client,
        "textDocument/definition",
        &main,
        source,
        "String",
    )
    .await;
    assert_eq!(definition["uri"], "lspkl://stdlib/base.pkl");
    let contents = client
        .request("pkl/fileContents", json!({"uri":definition["uri"]}))
        .await;
    let text = contents["result"].as_str().unwrap();
    let start = definition["range"]["start"]["line"].as_u64().unwrap() as usize;
    let end = definition["range"]["end"]["line"].as_u64().unwrap() as usize;
    let declaration = text
        .lines()
        .skip(start)
        .take(end - start + 1)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(declaration.contains("class String"), "{declaration}");
    assert_eq!(text, pkl_vfs::stdlib::source("base").unwrap());
    let missing = client
        .request("pkl/fileContents", json!({"uri":"pkl:nonexistent"}))
        .await;
    assert_eq!(missing["error"]["code"], -32603);
    assert!(missing.get("result").is_none());
    client.shutdown().await;
}

#[tokio::test]
async fn modulepath_archive_precedes_directory_for_completion_and_navigation() {
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("lib.zip");
    let directory = temp.path().join("lib");
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join("target.pkl"),
        "/// Later directory docs.\nvalue: String = \"later\"\n",
    )
    .unwrap();
    let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    writer
        .start_file("target.pkl", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer
        .write_all(b"/// Earlier archive docs.\nmodule fixture.archive\nvalue: Int = 7\n")
        .unwrap();
    writer.finish().unwrap();
    let main = uri(&temp.path().join("main.pkl"));
    // Do not advertise workspace/configuration here: defaults must not overwrite
    // these explicit initialization modulepaths.
    let mut client = LspClient::spawn(Path::new(env!("CARGO_BIN_EXE_lspkl")), temp.path()).await;
    let initialized = client
        .request(
            "initialize",
            json!({"capabilities":{},"rootUri":uri(temp.path()),
        "initializationOptions":{"modulepath":[archive,directory]}}),
        )
        .await;
    assert!(initialized.get("error").is_none(), "{initialized}");
    client.notify("initialized", json!({})).await;
    let source = "import \"modulepath:/target.pkl\" as target\nanswer = target.value\n";
    open(&mut client, &main, source).await;
    diagnostics(&mut client, &main).await;
    let location = at(
        &mut client,
        "textDocument/definition",
        &main,
        source,
        "value",
    )
    .await;
    assert!(
        location["uri"]
            .as_str()
            .unwrap()
            .starts_with("lspkl://jar/"),
        "{location}"
    );
    assert_eq!(location["range"], range(2, 0, 14));
    let contents = client
        .request("pkl/fileContents", json!({"uri":location["uri"]}))
        .await;
    assert!(
        contents["result"]
            .as_str()
            .unwrap()
            .contains("Earlier archive docs.")
    );
    client.notify("textDocument/didChange",json!({"textDocument":{"uri":main,"version":2},"contentChanges":[{"text":"import \"modulepath:/tar\""}]})).await;
    diagnostics(&mut client, &main).await;
    let response = client
        .request(
            "textDocument/completion",
            json!({"textDocument":{"uri":main},"position":{"line":0,"character":23}}),
        )
        .await;
    let items = response["result"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{response}");
    assert_eq!(items[0]["label"], "target.pkl");
    let target =
        fluent_uri::Uri::parse(items[0]["data"]["targetUri"].as_str().unwrap().to_owned()).unwrap();
    assert_eq!(
        pkl_vfs::to_editor_uri(&target).unwrap().as_str(),
        location["uri"].as_str().unwrap()
    );
    let resolved = client
        .request("completionItem/resolve", items[0].clone())
        .await;
    let docs = resolved["result"]["documentation"]["value"]
        .as_str()
        .unwrap();
    assert!(
        docs.contains("Earlier archive docs.") && !docs.contains("Later directory docs."),
        "{docs}"
    );
    client.shutdown().await;
}

#[tokio::test]
async fn close_reopen_during_edits_publishes_and_serves_the_new_epoch() {
    let temp = tempfile::tempdir().unwrap();
    let main = uri(&temp.path().join("reopened.pkl"));
    let mut client = start(temp.path(), json!({})).await;
    let old_source = "value: Int = \"wrong\"\nanswer = value\n";
    open(&mut client, &main, old_source).await;
    assert!(
        !diagnostics(&mut client, &main).await["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for version in 2..42 {
        client.notify("textDocument/didChange", json!({"textDocument":{"uri":main,"version":version},"contentChanges":[{"text":old_source}]})).await;
    }
    client
        .notify(
            "textDocument/didClose",
            json!({"textDocument":{"uri":main}}),
        )
        .await;
    let source = "value: String = \"reopened\"\nanswer = value\n";
    open(&mut client, &main, source).await;
    loop {
        let published = diagnostics(&mut client, &main).await;
        if published["version"] == 1 && published["diagnostics"] == json!([]) {
            break;
        }
    }
    let hover = at(&mut client, "textDocument/hover", &main, source, "value").await;
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("value: String"),
        "{hover}"
    );
    let contents = client
        .request("pkl/fileContents", json!({"uri":main}))
        .await;
    assert_eq!(contents["result"], source);
    client.shutdown().await;
}

#[tokio::test]
async fn typed_syntax_uses_if_branches_and_real_null_safe_tokens() {
    let temp = tempfile::tempdir().unwrap();
    let main = uri(&temp.path().join("typed-syntax.pkl"));
    let source = concat!(
        "c = true\n",
        "choice = if (c) 1 else \"a\"\n",
        "receiver: String = \"hello\"\n",
        "ordinary = receiver /* ?. is only a comment */ . /* gap */ length\n",
        "safe = receiver /* before */ ?. /* after */ length\n",
        "chosen = choice\n",
        "ordinaryValue = ordinary\n",
        "safeValue = safe\n",
    );
    let mut client = start(temp.path(), json!({})).await;
    open(&mut client, &main, source).await;
    let published = diagnostics(&mut client, &main).await;
    assert_eq!(published["diagnostics"], json!([]), "{published}");
    for (name, expected) in [
        ("choice", "choice: Int|\"a\""),
        ("ordinary", "ordinary: Int"),
        ("safe", "safe: Int?"),
    ] {
        let hover = at(&mut client, "textDocument/hover", &main, source, name).await;
        let declaration = hover["contents"]["value"]
            .as_str()
            .unwrap_or_else(|| panic!("missing hover for {name}: {hover}"))
            .split("\n")
            .nth(1)
            .unwrap();
        assert_eq!(declaration, expected, "{hover}");
    }
    client.shutdown().await;
}

#[tokio::test]
async fn watched_disk_changes_republish_importer_diagnostics_and_recover_missing_targets() {
    let temp = tempfile::tempdir().unwrap();
    let target_path = temp.path().join("library.pkl");
    let target = uri(&target_path);
    let main = uri(&temp.path().join("main.pkl"));
    let source = "import \"./library.pkl\" as library\nanswer = library.value\n";
    fs::write(&target_path, "value: Int = 1\n").unwrap();
    let mut client = LspClient::spawn(Path::new(env!("CARGO_BIN_EXE_lspkl")), temp.path()).await;
    let response = client
        .request(
            "initialize",
            json!({
                "rootUri":uri(temp.path()),
                "workspaceFolders":[{"uri":uri(temp.path()),"name":"fixture"}],
                "capabilities":{"workspace":{"didChangeWatchedFiles":{"dynamicRegistration":true}}}
            }),
        )
        .await;
    assert!(response.get("error").is_none(), "{response}");
    client.notify("initialized", json!({})).await;
    let registration = client.notification("client/registerCapability").await;
    let registrations = registration["params"]["registrations"].as_array().unwrap();
    let watchers = registrations
        .iter()
        .find(|entry| entry["method"] == "workspace/didChangeWatchedFiles")
        .unwrap()["registerOptions"]["watchers"]
        .as_array()
        .unwrap();
    let mut patterns: Vec<_> = watchers
        .iter()
        .map(|watcher| watcher["globPattern"].as_str().unwrap())
        .collect();
    patterns.sort_unstable();
    assert_eq!(
        patterns,
        ["**/*.pkl", "**/PklProject", "**/PklProject.deps.json"]
    );
    for watcher in watchers {
        assert_eq!(watcher["kind"], 7);
    }
    open(&mut client, &main, source).await;
    assert_eq!(
        diagnostics(&mut client, &main).await["diagnostics"],
        json!([])
    );
    // The importer stays at version 1 throughout: only watched disk inputs change.
    for (kind, text, expect_error) in [
        (2, Some("other: String = \"changed\"\n"), true),
        (2, Some("value: String = \"recovered\"\n"), false),
        (3, None, true),
        (1, Some("value: Int = 42\n"), false),
    ] {
        if let Some(text) = text {
            fs::write(&target_path, text).unwrap();
        } else {
            fs::remove_file(&target_path).unwrap();
        }
        client
            .notify(
                "workspace/didChangeWatchedFiles",
                json!({"changes":[{"uri":target,"type":kind}]}),
            )
            .await;
        let published = diagnostics(&mut client, &main).await;
        assert_eq!(published["version"], 1, "{published}");
        let errors = published["diagnostics"].as_array().unwrap();
        if expect_error {
            if kind == 2 {
                assert_eq!(errors.len(), 1, "{published}");
                assert_eq!(errors[0]["range"], range(1, 17, 22));
            } else {
                assert_eq!(errors.len(), 1, "{published}");
                assert_eq!(errors[0]["range"], range(0, 7, 22));
            }
        } else {
            assert!(errors.is_empty(), "{published}");
        }
    }
    let hover = at(&mut client, "textDocument/hover", &main, source, "value").await;
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("value: Int"),
        "{hover}"
    );
    client.shutdown().await;
}

#[tokio::test]
async fn watched_archive_changes_refresh_present_and_missing_members() {
    fn write_archive(path: &Path, value: &str, include_future: bool) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        zip.start_file("existing.pkl", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(value.as_bytes()).unwrap();
        if include_future {
            zip.start_file("future.pkl", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"value = 1\n").unwrap();
        }
        zip.finish().unwrap();
    }
    let temp = tempfile::tempdir().unwrap();
    // The backing archive is covered by the registered **/*.pkl watch pattern.
    let archive = temp.path().join("library.pkl");
    write_archive(&archive, "value: Int = 1\n", false);
    let mut client = LspClient::spawn(Path::new(env!("CARGO_BIN_EXE_lspkl")), temp.path()).await;
    let initialized = client
        .request(
            "initialize",
            json!({
                "rootUri":uri(temp.path()),
                "capabilities":{"workspace":{"didChangeWatchedFiles":{"dynamicRegistration":true}}},
                "initializationOptions":{"pkl":{"modulepath":[archive]}}
            }),
        )
        .await;
    assert!(initialized.get("error").is_none(), "{initialized}");
    client.notify("initialized", json!({})).await;
    let main = uri(&temp.path().join("main.pkl"));
    let source = "import \"modulepath:/existing.pkl\" as existing\nimport \"modulepath:/future.pkl\" as future\nanswer = existing.value\n";
    open(&mut client, &main, source).await;
    assert_eq!(
        diagnostics(&mut client, &main).await["diagnostics"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    write_archive(&archive, "value: String = \"updated\"\n", true);
    client
        .notify(
            "workspace/didChangeWatchedFiles",
            json!({"changes":[{"uri":uri(&archive),"type":2}]}),
        )
        .await;
    assert_eq!(
        diagnostics(&mut client, &main).await["diagnostics"],
        json!([])
    );
    let hover = at(&mut client, "textDocument/hover", &main, source, "value").await;
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("value: String"),
        "{hover}"
    );
    fs::remove_file(&archive).unwrap();
    client
        .notify(
            "workspace/didChangeWatchedFiles",
            json!({"changes":[{"uri":uri(&archive),"type":3}]}),
        )
        .await;
    assert_eq!(
        diagnostics(&mut client, &main).await["diagnostics"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    write_archive(&archive, "value: Boolean = true\n", true);
    client
        .notify(
            "workspace/didChangeWatchedFiles",
            json!({"changes":[{"uri":uri(&archive),"type":1}]}),
        )
        .await;
    assert_eq!(
        diagnostics(&mut client, &main).await["diagnostics"],
        json!([])
    );
    let hover = at(&mut client, "textDocument/hover", &main, source, "value").await;
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("value: Boolean"),
        "{hover}"
    );
    client.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn slow_formatters_do_not_delay_edits_cancellation_or_fresh_hover() {
    use std::{os::unix::fs::PermissionsExt, time::Duration};
    let temp = tempfile::tempdir().unwrap();
    let started = temp.path().join("started");
    let release = temp.path().join("release");
    let binary = temp.path().join("formatter");
    fs::write(&binary, format!("#!/bin/sh\necho started >> '{}'\nwhile [ ! -e '{}' ]; do /bin/sleep 0.01; done\n/bin/cat\n", started.display(), release.display())).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let mut client =
        LspClient::spawn_manual_configuration(Path::new(env!("CARGO_BIN_EXE_lspkl")), temp.path())
            .await;
    let response = client
        .request(
            "initialize",
            json!({"capabilities":{},"initializationOptions":{"cli":{"path":binary}}}),
        )
        .await;
    assert!(response.get("error").is_none(), "{response}");
    client.notify("initialized", json!({})).await;
    let main = uri(&temp.path().join("main.pkl"));
    open(&mut client, &main, "value: Int = 1\nanswer = value\n").await;
    let sender = client.sender();
    let mut requests = Vec::new();
    for _ in 0..4 {
        requests.push(sender.start_request("textDocument/formatting",json!({"textDocument":{"uri":main},"options":{"tabSize":2,"insertSpaces":true}})).await);
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if fs::read_to_string(&started)
                .unwrap_or_default()
                .lines()
                .count()
                == 4
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("four external formatter jobs must start");
    client.notify("textDocument/didChange",json!({"textDocument":{"uri":main,"version":2},"contentChanges":[{"text":"value: String = \"fresh\"\nanswer = value\n"}]})).await;
    client
        .notify("$/cancelRequest", json!({"id":requests[0].id}))
        .await;
    let cancelled =
        tokio::time::timeout(Duration::from_millis(500), client.response(requests[0].id))
            .await
            .expect("cancel is not queued behind formatter work");
    assert_eq!(cancelled["error"]["code"], -32800, "{cancelled}");
    let hover = tokio::time::timeout(
        Duration::from_millis(500),
        client.request(
            "textDocument/hover",
            json!({"textDocument":{"uri":main},"position":{"line":1,"character":12}}),
        ),
    )
    .await
    .expect("hover is not queued behind external work");
    assert!(
        hover["result"]["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("value: String"),
        "{hover}"
    );
    fs::write(&release, "").unwrap();
    for request in &requests[1..] {
        let response = client.response(request.id).await;
        assert_eq!(response["error"]["code"], -32801, "{response}");
    }
    // The completed cancelled job must not produce a second response.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(50);
    while let Some(message) = client.observe_until(deadline).await {
        assert_ne!(
            message.message["id"], requests[0].id,
            "duplicate cancelled response"
        );
    }
    client.shutdown().await;
}

#[tokio::test]
async fn stale_configuration_reply_cannot_revert_pushed_modulepath() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("old");
    let new = temp.path().join("new");
    fs::create_dir(&old).unwrap();
    fs::create_dir(&new).unwrap();
    fs::write(old.join("library.pkl"), "value: Int = 1\n").unwrap();
    fs::write(new.join("library.pkl"), "value: String = \"new\"\n").unwrap();
    let mut client =
        LspClient::spawn_manual_configuration(Path::new(env!("CARGO_BIN_EXE_lspkl")), temp.path())
            .await;
    let response = client
        .request(
            "initialize",
            json!({"capabilities":{"workspace":{"configuration":true}}}),
        )
        .await;
    assert!(response.get("error").is_none(), "{response}");
    client.notify("initialized", json!({})).await;
    let pull = client.notification("workspace/configuration").await;
    client
        .notify(
            "workspace/didChangeConfiguration",
            json!({"settings":{"modulepath":[new]}}),
        )
        .await;
    let main = uri(&temp.path().join("main.pkl"));
    let source = "import \"modulepath:/library.pkl\" as library\nanswer = library.value\n";
    open(&mut client, &main, source).await;
    diagnostics(&mut client, &main).await;
    client
        .respond(pull["id"].clone(), json!([null, null, [old], [], null]))
        .await;
    let hover = at(&mut client, "textDocument/hover", &main, source, "value").await;
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("value: String"),
        "{hover}"
    );
    client.shutdown().await;
}
#[cfg(unix)]
async fn blocked_project_job(watched: Option<&str>, cancel: bool) {
    use std::{os::unix::fs::PermissionsExt, time::Duration};
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("PklProject");
    fs::write(&project, "amends \"pkl:Project\"\n").unwrap();
    let started = temp.path().join("started");
    let release = temp.path().join("release");
    let old = temp.path().join("old-cli");
    let new = temp.path().join("new-cli");
    let metadata = |version: &str| {
        json!({
        "projectFileUri":uri(&project),"packageUri":format!("package://example.com/app@{version}"),
        "declaredDependencies":{},"evaluatorSettings":null
    }).to_string()
    };
    fs::write(&old, format!("#!/bin/sh\ncase \"$1\" in\nproject) exit 0 ;;\neval) echo started > '{}'\nwhile [ ! -e '{}' ]; do /bin/sleep 0.01; done\nprintf '%s' '{}' ;;\n*) exit 9 ;;\nesac\n",started.display(),release.display(),metadata("1.0.0"))).unwrap();
    fs::write(&new, format!("#!/bin/sh\ncase \"$1\" in\nproject) exit 0 ;;\neval) printf '%s' '{}' ;;\n*) exit 9 ;;\nesac\n",metadata("2.0.0"))).unwrap();
    for script in [&old, &new] {
        fs::set_permissions(script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut client = start(temp.path(), json!({"cli":{"path":old}})).await;
    let request = client
        .sender()
        .start_request("pkl/syncProjects", json!(null))
        .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("sync job entered CLI eval");
    let state = temp.path().join(".lspkl/projects.json");
    assert!(!state.exists(), "unaccepted job must not persist");
    if let Some(name) = watched {
        client
            .notify(
                "workspace/didChangeWatchedFiles",
                json!({"changes":[{"uri":uri(&temp.path().join(name)),"type":2}]}),
            )
            .await;
        // A response is a protocol-order barrier, not a timing assumption about the I/O worker.
        let barrier = client.request("textDocument/hover",json!({"textDocument":{"uri":uri(&temp.path().join("missing.pkl"))},"position":{"line":0,"character":0}})).await;
        assert!(barrier.get("error").is_none(), "{barrier}");
        fs::write(&release, "").unwrap();
        let response = client.response(request.id).await;
        assert_eq!(response["error"]["code"], -32801, "{response}");
        client.shutdown().await;
        assert!(
            !state.exists(),
            "watcher-rejected unregistered project must not persist"
        );
    } else {
        if cancel {
            client
                .notify("$/cancelRequest", json!({"id":request.id}))
                .await;
            let response = client.response(request.id).await;
            assert_eq!(response["error"]["code"], -32800, "{response}");
        }
        client
            .notify(
                "workspace/didChangeConfiguration",
                json!({"settings":{"cli":{"path":new}}}),
            )
            .await;
        let accepted = client.request("pkl/syncProjects", json!(null)).await;
        assert!(accepted.get("error").is_none(), "{accepted}");
        fs::write(&release, "").unwrap();
        if !cancel {
            let response = client.response(request.id).await;
            assert_eq!(response["error"]["code"], -32801, "{response}");
        }
        client.shutdown().await;
        let saved = fs::read_to_string(state).unwrap();
        assert!(saved.contains("app@2.0.0"), "{saved}");
        assert!(
            !saved.contains("app@1.0.0"),
            "stale job overwrote accepted state: {saved}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn project_watchers_invalidate_jobs_before_manager_registration() {
    for name in ["PklProject", "PklProject.deps.json"] {
        blocked_project_job(Some(name), false).await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn superseded_sync_cannot_overwrite_accepted_persistence() {
    blocked_project_job(None, false).await;
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_sync_cannot_overwrite_accepted_persistence() {
    blocked_project_job(None, true).await;
}

#[tokio::test]
async fn uncached_https_import_resolves_and_clears_diagnostics_without_file_contents() {
    let temp = tempfile::tempdir().unwrap();
    let ca = temp.path().join("fixture-ca.pem");
    let mut origin = https::HttpsFixture::start(&ca);
    let cache = temp.path().join("cache");
    let imported = fluent_uri::Uri::parse(origin.uri.clone()).unwrap();
    let loader = pkl_vfs::StandardSourceLoader::new();
    assert!(
        pkl_vfs::SourceLoader::load(&loader, &imported, &cache).is_err(),
        "the HTTPS import must start uncached"
    );
    let main = uri(&temp.path().join("main.pkl"));
    let source = format!(
        "import \"{}\" as library\nanswer = library.value\n",
        origin.uri
    );
    let mut client = LspClient::spawn_with_env(
        Path::new(env!("CARGO_BIN_EXE_lspkl")),
        temp.path(),
        &[("SSL_CERT_FILE", ca.as_path())],
    )
    .await;
    let response = client
        .request(
            "initialize",
            json!({
                "processId":null,"rootUri":uri(temp.path()),
                "workspaceFolders":[{"uri":uri(temp.path()),"name":"fixture"}],
                "capabilities":{"workspace":{"configuration":true}},
                "initializationOptions":{"cacheDir":cache}
            }),
        )
        .await;
    assert!(response.get("error").is_none(), "{response}");
    client.notify("initialized", json!({})).await;
    open(&mut client, &main, &source).await;
    let cold = diagnostics(&mut client, &main).await;
    assert!(
        !cold["diagnostics"].as_array().unwrap().is_empty(),
        "{cold}"
    );
    origin.release();
    let cleared = loop {
        let published = diagnostics(&mut client, &main).await;
        if published["diagnostics"].as_array().unwrap().is_empty() {
            break published;
        }
    };
    assert_eq!(cleared["version"], 1);
    assert!(
        pkl_vfs::SourceLoader::load(&loader, &imported, &cache).is_ok(),
        "semantic I/O must populate the HTTPS cache"
    );
    let definition = at(
        &mut client,
        "textDocument/definition",
        &main,
        &source,
        "value",
    )
    .await;
    assert_eq!(
        definition["uri"],
        pkl_vfs::to_editor_uri(&imported).unwrap().as_str()
    );
    assert_eq!(
        definition["range"],
        json!({"start":{"line":0,"character":0},"end":{"line":1,"character":15}})
    );
    let hover = at(&mut client, "textDocument/hover", &main, &source, "value").await;
    let markdown = hover["contents"]["value"].as_str().unwrap();
    assert!(markdown.contains("value: Int"), "{markdown}");
    assert!(
        markdown.contains("Downloaded HTTPS documentation."),
        "{markdown}"
    );
    client.shutdown().await;
}

#[tokio::test]
async fn blocked_import_does_not_freeze_unrelated_hover_or_waiting_cancellation() {
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let ca = temp.path().join("ca.pem");
    let origin = https::HttpsFixture::start(&ca);
    let mut client = LspClient::spawn_with_env(
        Path::new(env!("CARGO_BIN_EXE_lspkl")),
        temp.path(),
        &[("SSL_CERT_FILE", ca.as_path())],
    )
    .await;
    assert!(client.request("initialize", json!({"capabilities":{},"initializationOptions":{"cacheDir":temp.path().join("cache")}})).await.get("error").is_none());
    client.notify("initialized", json!({})).await;
    let remote = uri(&temp.path().join("remote.pkl"));
    let source = format!(
        "import \"{}\" as library\nanswer = library.value\n",
        origin.uri
    );
    open(&mut client, &remote, &source).await;
    origin.wait_requested();
    let waiting = client
        .sender()
        .start_request(
            "textDocument/hover",
            json!({"textDocument":{"uri":remote},"position":position(&source,"value")}),
        )
        .await;
    let local = uri(&temp.path().join("local.pkl"));
    open(&mut client, &local, "value: Int = 42\nanswer = value\n").await;
    let hover = tokio::time::timeout(
        Duration::from_secs(2),
        client.request(
            "textDocument/hover",
            json!({"textDocument":{"uri":local},"position":{"line":1,"character":10}}),
        ),
    )
    .await
    .expect("unrelated hover must not wait for HTTPS");
    assert!(
        hover["result"]["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("Int"),
        "{hover}"
    );
    client
        .notify("$/cancelRequest", json!({"id":waiting.id}))
        .await;
    let cancelled = tokio::time::timeout(Duration::from_secs(1), client.response(waiting.id))
        .await
        .expect("waiting cancellation must respond promptly");
    assert_eq!(cancelled["error"]["code"], -32800, "{cancelled}");
    client.shutdown().await;
}
