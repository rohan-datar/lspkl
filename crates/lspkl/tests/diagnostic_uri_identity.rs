mod support;

use fluent_uri::Uri;
use lspkl::Workspace;
use serde_json::json;
use std::{path::Path, time::Duration};
use support::LspClient;
use tokio::time::Instant;

fn open(workspace: &mut Workspace, uri: &str) {
    workspace
        .apply_open(lsp_types::DidOpenTextDocumentParams {
            text_document: lsp_types::TextDocumentItem {
                uri: uri.parse().unwrap(),
                language_id: "pkl".into(),
                version: 1,
                text: "value: Int = 1\n".into(),
            },
        })
        .unwrap();
}

#[test]
fn client_aliases_share_canonical_inputs_but_close_independently() {
    for canonical in [
        "pkl:base",
        "https://example.invalid/library.pkl",
        "jar:file:///nonexistent/library.zip!/library.pkl",
    ] {
        let canonical = Uri::parse(canonical.to_owned()).unwrap();
        let editor = pkl_vfs::to_editor_uri(&canonical).unwrap();
        let mut workspace = Workspace::new();
        open(&mut workspace, editor.as_str());
        assert_eq!(
            workspace.open_document_uris(),
            vec![editor.as_str().parse().unwrap()]
        );
        let file = workspace.document(editor.as_str()).unwrap();
        let text = file.text(&workspace.db).clone();
        open(&mut workspace, canonical.as_str());
        assert_eq!(workspace.document(canonical.as_str()), Some(file));
        assert_eq!(workspace.open_document_uris().len(), 2);
        let work = workspace
            .apply_close(lsp_types::DidCloseTextDocumentParams {
                text_document: lsp_types::TextDocumentIdentifier {
                    uri: editor.as_str().parse().unwrap(),
                },
            })
            .unwrap();
        workspace.apply_reload(work.load());
        assert_eq!(
            workspace.open_document_uris(),
            vec![canonical.as_str().parse().unwrap()]
        );
        assert_eq!(workspace.document(canonical.as_str()), Some(file));
        assert_eq!(file.text(&workspace.db), &text);
        assert_eq!(file.version(&workspace.db), Some(1));
        if canonical.scheme().as_str() == "pkl" {
            assert_ne!(text.as_ref(), "value: Int = 1\n", "SDK text is immutable");
        }
    }
}

#[tokio::test]
async fn stdio_publishes_exact_client_alias_and_stops_closed_alias_worker() {
    let temp = tempfile::tempdir().unwrap();
    let mut client = LspClient::spawn(Path::new(env!("CARGO_BIN_EXE_lspkl")), temp.path()).await;
    let response = client
        .request("initialize", json!({"processId":null,"capabilities":{}}))
        .await;
    assert!(response.get("error").is_none(), "{response}");
    client.notify("initialized", json!({})).await;

    for canonical in [
        "pkl:base",
        "https://example.invalid/library.pkl",
        "jar:file:///nonexistent/library.zip!/library.pkl",
    ] {
        let canonical = Uri::parse(canonical.to_owned()).unwrap();
        let editor = pkl_vfs::to_editor_uri(&canonical).unwrap();
        client
            .notify(
                "textDocument/didOpen",
                json!({"textDocument":{
                    "uri":editor.as_str(),"languageId":"pkl","version":1,"text":"value: Int = 1\n"
                }}),
            )
            .await;
        let published = client.notification("textDocument/publishDiagnostics").await;
        assert_eq!(published["params"]["uri"], editor.as_str(), "{published}");
        assert_eq!(published["params"]["version"], 1, "{published}");
        if canonical.scheme().as_str() != "pkl" {
            assert_eq!(published["params"]["diagnostics"], json!([]), "{published}");
        }
        // Remove this document before testing the next scheme. No backing source exists
        // for HTTPS/jar; closing must not require a network or an archive fixture.
        client
            .notify(
                "textDocument/didClose",
                json!({"textDocument":{"uri":editor.as_str()}}),
            )
            .await;
        let deadline = Instant::now() + Duration::from_millis(100);
        while let Some(observed) = client.observe_until(deadline).await {
            assert_ne!(
                observed.message["method"], "textDocument/publishDiagnostics",
                "{:?}",
                observed.message
            );
        }
    }

    for uri in ["lspkl://stdlib/base.pkl", "pkl:base"] {
        client
            .notify(
                "textDocument/didOpen",
                json!({"textDocument":{
                    "uri":uri,"languageId":"pkl","version":1,"text":"ignored SDK editor text"
                }}),
            )
            .await;
        let mut seen = std::collections::HashSet::new();
        let expected = if uri == "pkl:base" { 2 } else { 1 };
        while seen.len() < expected {
            let published = client.notification("textDocument/publishDiagnostics").await;
            seen.insert(published["params"]["uri"].as_str().unwrap().to_owned());
        }
        assert!(seen.contains(uri));
    }
    client
        .notify(
            "textDocument/didClose",
            json!({"textDocument":{"uri":"lspkl://stdlib/base.pkl"}}),
        )
        .await;
    let published = client.notification("textDocument/publishDiagnostics").await;
    assert_eq!(published["params"]["uri"], "pkl:base", "{published}");
    client
        .notify(
            "workspace/didChangeConfiguration",
            json!({"settings":{"modulepath":[]}}),
        )
        .await;
    let published = client.notification("textDocument/publishDiagnostics").await;
    assert_eq!(
        published["params"]["uri"], "pkl:base",
        "closed alias worker survived: {published}"
    );
    let deadline = Instant::now() + Duration::from_millis(100);
    while let Some(observed) = client.observe_until(deadline).await {
        assert_ne!(
            observed.message["method"], "textDocument/publishDiagnostics",
            "{:?}",
            observed.message
        );
    }
    client.shutdown().await;
}
