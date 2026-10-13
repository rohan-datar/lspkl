//! Position negotiation and Unicode ranges over the actual stdio executable.
mod support;

use std::{fs, path::Path};

use serde_json::{Value, json};
use support::LspClient;

fn uri(path: &Path) -> String {
    pkl_vfs::path_to_uri(path).unwrap().into_string()
}

// This is the client's independent wire encoding, not the server's converter.
fn position(source: &str, offset: usize, utf8: bool) -> Value {
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|&byte| byte == b'\n').count();
    let tail = prefix.rsplit('\n').next().unwrap();
    let character = if utf8 {
        tail.len()
    } else {
        tail.encode_utf16().count()
    };
    json!({"line":line,"character":character})
}

fn range(source: &str, start: usize, end: usize, utf8: bool) -> Value {
    json!({"start":position(source,start,utf8),"end":position(source,end,utf8)})
}

async fn request(client: &mut LspClient, method: &str, params: Value) -> Value {
    let mut response = client.request(method, params.clone()).await;
    if response.get("error").is_some() {
        assert_eq!(response["error"]["code"], -32801, "{response}");
        response = client.request(method, params).await;
    }
    assert!(
        response.get("error").is_none(),
        "quiescent retry failed: {response}"
    );
    response["result"].clone()
}

async fn start(workspace: &Path, offered: Option<Value>, expected: &str) -> LspClient {
    let mut client = LspClient::spawn(Path::new(env!("CARGO_BIN_EXE_lspkl")), workspace).await;
    let mut capabilities = json!({});
    if let Some(encodings) = offered {
        capabilities["general"] = json!({"positionEncodings":encodings});
    }
    let initialized = request(
        &mut client,
        "initialize",
        json!({
            "processId":null,"rootUri":uri(workspace),"capabilities":capabilities
        }),
    )
    .await;
    assert_eq!(initialized["capabilities"]["positionEncoding"], expected);
    client.notify("initialized", json!({})).await;
    client
}

async fn open(client: &mut LspClient, document: &str, source: &str) {
    client
        .notify(
            "textDocument/didOpen",
            json!({"textDocument":{
                "uri":document,"languageId":"pkl","version":1,"text":source
            }}),
        )
        .await;
}

async fn diagnostics(client: &mut LspClient, document: &str, version: i32) -> Value {
    loop {
        let published = client.notification("textDocument/publishDiagnostics").await;
        if published["params"]["uri"] == document && published["params"]["version"] == version {
            return published["params"]["diagnostics"].clone();
        }
    }
}

async fn at(
    client: &mut LspClient,
    method: &str,
    document: &str,
    source: &str,
    offset: usize,
    utf8: bool,
) -> Value {
    request(
        client,
        method,
        json!({"textDocument":{"uri":document},"position":position(source,offset,utf8)}),
    )
    .await
}

#[tokio::test]
async fn unicode_hover_definitions_diagnostics_and_literal_edits_use_negotiated_encoding() {
    for utf8 in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        let foreign_source = "/// Foreign documentation.\ntarget: Int = \"é😀\".length\n";
        fs::write(workspace.path().join("foreign.pkl"), foreign_source).unwrap();
        let offered = utf8.then(|| json!(["utf-16", "utf-8"]));
        let mut client = start(
            workspace.path(),
            offered,
            if utf8 { "utf-8" } else { "utf-16" },
        )
        .await;
        let document = uri(&workspace.path().join("main.pkl"));
        let source = concat!(
            "import \"foreign.pkl\" as foreign\n",
            "x: Int = \"ééééé😀\".length\n",
            "use = \"ééééé😀\".length + x\n",
            "other = \"ééééé😀\".length + foreign.target\n",
            "broken = \"ééééé😀\".length + missing\n",
        );
        open(&mut client, &document, source).await;
        let published = diagnostics(&mut client, &document, 1).await;
        let error = published
            .as_array()
            .unwrap()
            .iter()
            .find(|diagnostic| {
                diagnostic["message"]
                    .as_str()
                    .unwrap()
                    .contains("unresolved reference: missing")
            })
            .unwrap();
        let missing = source.rfind("missing").unwrap();
        assert_eq!(
            error["range"],
            range(source, missing, missing + "missing".len(), utf8)
        );
        assert_eq!(error["severity"], 1);

        let cursor = source.find(" + x").unwrap() + 3;
        let hover = at(
            &mut client,
            "textDocument/hover",
            &document,
            source,
            cursor,
            utf8,
        )
        .await;
        assert!(
            hover["contents"]["value"]
                .as_str()
                .unwrap()
                .contains("x: Int"),
            "{hover}"
        );
        let definition = at(
            &mut client,
            "textDocument/definition",
            &document,
            source,
            cursor,
            utf8,
        )
        .await;
        let declaration_start = source.find("x: Int").unwrap();
        let declaration_end = declaration_start + source[declaration_start..].find('\n').unwrap();
        assert_eq!(
            definition,
            json!({"uri":document,"range":range(source,declaration_start,declaration_end,utf8)})
        );

        let foreign_cursor = source.find("foreign.target").unwrap() + "foreign.".len();
        let definition = at(
            &mut client,
            "textDocument/definition",
            &document,
            source,
            foreign_cursor,
            utf8,
        )
        .await;
        assert_eq!(
            definition,
            json!({
                "uri":uri(&workspace.path().join("foreign.pkl")),
                "range":range(foreign_source,0,foreign_source.len()-1,utf8)
            })
        );

        let literal_document = uri(&workspace.path().join("literal.pkl"));
        let literal_source = "choice: \"é😀\"|\"é😃\" = \"é\"\n";
        open(&mut client, &literal_document, literal_source).await;
        let _ = diagnostics(&mut client, &literal_document, 1).await;
        let end = literal_source.rfind('"').unwrap();
        let start = literal_source.rfind(" = \"").unwrap() + 4;
        let completed = at(
            &mut client,
            "textDocument/completion",
            &literal_document,
            literal_source,
            end,
            utf8,
        )
        .await;
        let items = completed
            .as_array()
            .or_else(|| completed["items"].as_array())
            .unwrap();
        let item = items
            .iter()
            .find(|item| item["label"] == "\"é😀\"")
            .unwrap();
        assert_eq!(
            item["textEdit"]["range"],
            range(literal_source, start, end, utf8)
        );
        assert_eq!(item["textEdit"]["newText"], "é😀");
        let edited = format!(
            "{}{}{}",
            &literal_source[..start],
            item["textEdit"]["newText"].as_str().unwrap(),
            &literal_source[end..]
        );
        client.notify("textDocument/didChange", json!({
            "textDocument":{"uri":literal_document,"version":2},"contentChanges":[{"text":edited}]
        })).await;
        assert_eq!(
            diagnostics(&mut client, &literal_document, 2).await,
            json!([])
        );
        client.shutdown().await;
    }
}

#[tokio::test]
async fn module_uri_completion_replaces_unicode_contents_without_removing_quotes() {
    for utf8 in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        let directory = workspace.path().join("é😀");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("target.pkl"), "value = 42\n").unwrap();
        let mut client = start(
            workspace.path(),
            utf8.then(|| json!(["utf-8"])),
            if utf8 { "utf-8" } else { "utf-16" },
        )
        .await;
        let document = uri(&workspace.path().join("main.pkl"));
        let source = "import \"./é😀/ta\"\n";
        open(&mut client, &document, source).await;
        let _ = diagnostics(&mut client, &document, 1).await;
        let start = source.find('"').unwrap() + 1;
        let end = source.rfind('"').unwrap();
        let completed = at(
            &mut client,
            "textDocument/completion",
            &document,
            source,
            end,
            utf8,
        )
        .await;
        let items = completed
            .as_array()
            .or_else(|| completed["items"].as_array())
            .unwrap();
        let item = items
            .iter()
            .find(|item| item["label"] == "target.pkl")
            .unwrap();
        assert_eq!(item["textEdit"]["range"], range(source, start, end, utf8));
        let edited = format!(
            "{}{}{}",
            &source[..start],
            item["textEdit"]["newText"].as_str().unwrap(),
            &source[end..]
        );
        client
            .notify(
                "textDocument/didChange",
                json!({
                    "textDocument":{"uri":document,"version":2},"contentChanges":[{"text":edited}]
                }),
            )
            .await;
        assert_eq!(diagnostics(&mut client, &document, 2).await, json!([]));
        client.shutdown().await;
    }
}

#[tokio::test]
async fn explicit_utf16_and_unsupported_encodings_fall_back_to_utf16() {
    for offered in [json!(["utf-16"]), json!(["utf-32"])] {
        let workspace = tempfile::tempdir().unwrap();
        start(workspace.path(), Some(offered), "utf-16")
            .await
            .shutdown()
            .await;
    }
}
