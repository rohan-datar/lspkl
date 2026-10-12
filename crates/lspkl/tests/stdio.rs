//! Navigation, hover, completion, and lifecycle semantics over the shipped binary.
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
    let mut response = client
        .request(
            method,
            json!({"textDocument":{"uri":uri},"position":position(source,token)}),
        )
        .await;
    assert!(response.get("error").is_none(), "{response}");
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
