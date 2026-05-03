//! Lifecycle tests: initialize → didOpen → didChange → didClose → shutdown.

use lspkl::PklBackend;
use tower_lsp_server::LanguageServer;
use tower_lsp_server::lsp_types::*;

fn pkl_url(name: &str) -> Uri {
    format!("inmemory:///{name}").parse().unwrap()
}

fn open_params(uri: Uri, contents: &str, version: i32) -> DidOpenTextDocumentParams {
    DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri,
            language_id: "pkl".into(),
            version,
            text: contents.into(),
        },
    }
}

#[tokio::test]
async fn initialize_advertises_text_document_sync_full() {
    let backend = PklBackend::for_testing();
    let result = backend
        .initialize(InitializeParams::default())
        .await
        .expect("initialize should succeed");

    let sync = result
        .capabilities
        .text_document_sync
        .expect("server must advertise text document sync");
    let kind = match sync {
        TextDocumentSyncCapability::Kind(k) => k,
        TextDocumentSyncCapability::Options(opts) => opts.change.expect("change kind"),
    };
    assert_eq!(kind, TextDocumentSyncKind::FULL);
}

#[tokio::test]
async fn initialize_advertises_all_features() {
    let backend = PklBackend::for_testing();
    let result = backend
        .initialize(InitializeParams::default())
        .await
        .unwrap();
    let caps = result.capabilities;

    assert!(
        caps.hover_provider.is_some(),
        "hover provider must be advertised"
    );
    assert!(
        caps.definition_provider.is_some(),
        "definition provider must be advertised"
    );
    assert!(
        caps.completion_provider.is_some(),
        "completion provider must be advertised"
    );
    assert!(
        caps.code_action_provider.is_some(),
        "code action provider must be advertised"
    );
    assert!(
        caps.document_formatting_provider.is_some(),
        "formatting provider must be advertised"
    );
}

#[tokio::test]
async fn initialize_completion_trigger_chars() {
    // pkl-lsp advertises [".", "/", "\"", ":"] + a-z + A-Z.
    // This test asserts the dot trigger; expand once you've decided what to keep.
    let backend = PklBackend::for_testing();
    let result = backend
        .initialize(InitializeParams::default())
        .await
        .unwrap();
    let completion = match result.capabilities.completion_provider {
        Some(opts) => opts,
        None => panic!("completion provider must be advertised"),
    };
    let triggers = completion
        .trigger_characters
        .expect("completion needs trigger chars");
    assert!(
        triggers.iter().any(|c| c == "."),
        "`.` must be a completion trigger"
    );
}

#[tokio::test]
async fn did_open_then_state_has_file() {
    let backend = PklBackend::for_testing();
    let _ = backend
        .initialize(InitializeParams::default())
        .await
        .unwrap();

    let uri = pkl_url("a.pkl");
    backend
        .did_open(open_params(uri.clone(), "foo = 1", 1))
        .await;

    // state.file_manager should now know about the URL.
    let url = url::Url::parse(uri.as_str()).unwrap();
    let file = backend
        .state
        .file_manager
        .get(&url)
        .expect("file should be registered after did_open");
    assert_eq!(file.contents(), "foo = 1");
    assert_eq!(file.version(), Some(1));
}

#[tokio::test]
async fn did_change_updates_contents() {
    let backend = PklBackend::for_testing();
    let _ = backend
        .initialize(InitializeParams::default())
        .await
        .unwrap();

    let uri = pkl_url("b.pkl");
    backend
        .did_open(open_params(uri.clone(), "foo = 1", 1))
        .await;

    backend
        .did_change(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "foo = 2".into(),
            }],
        })
        .await;

    let url = url::Url::parse(uri.as_str()).unwrap();
    let file = backend.state.file_manager.get(&url).unwrap();
    assert_eq!(file.contents(), "foo = 2");
    assert_eq!(file.version(), Some(2));
}

#[tokio::test]
async fn did_close_removes_file_from_state() {
    let backend = PklBackend::for_testing();
    let _ = backend
        .initialize(InitializeParams::default())
        .await
        .unwrap();

    let uri = pkl_url("c.pkl");
    backend
        .did_open(open_params(uri.clone(), "foo = 1", 1))
        .await;

    backend
        .did_close(DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
        })
        .await;

    let url = url::Url::parse(uri.as_str()).unwrap();
    assert!(
        backend.state.file_manager.get(&url).is_none(),
        "file must be removed after did_close"
    );
}

#[tokio::test]
async fn shutdown_succeeds() {
    let backend = PklBackend::for_testing();
    let _ = backend
        .initialize(InitializeParams::default())
        .await
        .unwrap();
    backend.shutdown().await.expect("shutdown should succeed");
}
