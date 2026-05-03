//! Expectations for feature handlers.
//!
//! Right now, every feature handler returns `Ok(None)` — that's a valid LSP
//! response meaning "I have nothing for you here". These tests pin that
//! contract so a future regression (returning an error, or panicking) is
//! caught immediately. When you implement these features in later phases, the
//! tests here will need to be replaced with real assertions.

use lspkl::PklBackend;
use tower_lsp_server::LanguageServer;
use tower_lsp_server::lsp_types::*;

fn pkl_url(name: &str) -> Uri {
    format!("inmemory:///{name}").parse().unwrap()
}

fn position(line: u32, character: u32) -> Position {
    Position { line, character }
}

#[tokio::test]
async fn hover_returns_none() {
    let backend = PklBackend::for_testing();
    let result = backend
        .hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: pkl_url("a.pkl"),
                },
                position: position(0, 0),
            },
            work_done_progress_params: Default::default(),
        })
        .await
        .expect("hover must not return a JSON-RPC error in phase 2");
    assert!(result.is_none(), "phase-2 hover stub must return None");
}

#[tokio::test]
async fn goto_definition_returns_none() {
    let backend = PklBackend::for_testing();
    let result = backend
        .goto_definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: pkl_url("a.pkl"),
                },
                position: position(0, 0),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .await
        .expect("goto_definition must not error in phase 2");
    assert!(result.is_none());
}

#[tokio::test]
async fn completion_returns_none() {
    let backend = PklBackend::for_testing();
    let result = backend
        .completion(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: pkl_url("a.pkl"),
                },
                position: position(0, 0),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        })
        .await
        .expect("completion must not error in phase 2");
    assert!(result.is_none());
}

#[tokio::test]
async fn code_action_returns_none() {
    let backend = PklBackend::for_testing();
    let result = backend
        .code_action(CodeActionParams {
            text_document: TextDocumentIdentifier {
                uri: pkl_url("a.pkl"),
            },
            range: Range::new(position(0, 0), position(0, 0)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .await
        .expect("code_action must not error in phase 2");
    assert!(result.is_none());
}

#[tokio::test]
async fn formatting_returns_none() {
    let backend = PklBackend::for_testing();
    let result = backend
        .formatting(DocumentFormattingParams {
            text_document: TextDocumentIdentifier {
                uri: pkl_url("a.pkl"),
            },
            options: FormattingOptions {
                tab_size: 2,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
        })
        .await
        .expect("formatting must not error in phase 2");
    assert!(result.is_none());
}
