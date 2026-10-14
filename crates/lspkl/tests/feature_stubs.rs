//! Expectations for feature handlers.
//!
//! Every handler must return `Ok(None)` — a valid LSP response meaning "I
//! have nothing for you here" — when asked about a file the editor never
//! opened. These tests pin that contract so a regression (returning an
//! error, or panicking) is caught immediately. Handlers that are wired up
//! (hover, goto-definition) additionally get positive end-to-end tests that
//! `did_open` a document first.

use lsp_types::*;
use lspkl::{Workspace, handlers};

fn pkl_url(name: &str) -> Uri {
    format!("inmemory:///{name}").parse().unwrap()
}

fn position(line: u32, character: u32) -> Position {
    Position { line, character }
}

/// `did_open` a document so the wired handlers have something to look up.
fn open(backend: &mut Workspace, uri: &Uri, text: &str) {
    handlers::lifecycle::did_open(
        backend,
        DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "pkl".to_string(),
                version: 1,
                text: text.to_string(),
            },
        },
    )
    .unwrap();
}

#[test]
fn hover_returns_none() {
    let backend = Workspace::new();
    let result = handlers::documents::hover(
        &backend.snapshot(),
        HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: pkl_url("a.pkl"),
                },
                position: position(0, 0),
            },
            work_done_progress_params: Default::default(),
        },
    )
    .expect("hover must not return a JSON-RPC error");
    assert!(
        result.is_none(),
        "hover on an unopened file must return None"
    );
}

#[test]
fn hover_resolves_after_did_open() {
    let mut backend = Workspace::new();
    let uri = pkl_url("a.pkl");
    open(&mut backend, &uri, "x = 1\ny = x\n");

    let result = handlers::documents::hover(
        &backend.snapshot(),
        HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                // The `x` reference in `y = x`.
                position: position(1, 4),
            },
            work_done_progress_params: Default::default(),
        },
    )
    .expect("hover must not return a JSON-RPC error");
    let hover = result.expect("hover on a resolvable reference should return content");
    let HoverContents::Markup(markup) = hover.contents else {
        panic!("expected markup hover contents");
    };
    assert!(
        markup.value.contains("x"),
        "hover should mention the property; got: {}",
        markup.value,
    );
}

#[test]
fn goto_definition_returns_none() {
    let backend = Workspace::new();
    let result = handlers::documents::definition(
        &backend.snapshot(),
        GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: pkl_url("a.pkl"),
                },
                position: position(0, 0),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        },
    )
    .expect("goto_definition must not error");
    assert!(
        result.is_none(),
        "goto_definition on an unopened file must return None",
    );
}

#[test]
fn goto_definition_resolves_after_did_open() {
    let mut backend = Workspace::new();
    let uri = pkl_url("a.pkl");
    open(&mut backend, &uri, "x = 1\ny = x\n");

    let result = handlers::documents::definition(
        &backend.snapshot(),
        GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                // The `x` reference in `y = x`.
                position: position(1, 4),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        },
    )
    .expect("goto_definition must not error");
    let Some(GotoDefinitionResponse::Scalar(location)) = result else {
        panic!("expected a single Location, got {result:?}");
    };
    assert_eq!(location.uri, uri, "same-file target reuses the request URI");
    assert_eq!(
        location.range.start.line, 0,
        "declaration `x = 1` is on line 0",
    );
}

#[test]
fn completion_returns_none() {
    let backend = Workspace::new();
    let result = handlers::documents::completion(
        &backend.snapshot(),
        CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: pkl_url("a.pkl"),
                },
                position: position(0, 0),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        },
    )
    .expect("completion must not error in phase 2");
    assert!(result.is_none());
}

#[test]
fn code_action_returns_none() {
    let backend = Workspace::new();
    let result = handlers::documents::code_action(
        &backend.snapshot(),
        CodeActionParams {
            text_document: TextDocumentIdentifier {
                uri: pkl_url("a.pkl"),
            },
            range: Range::new(position(0, 0), position(0, 0)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        },
    )
    .expect("code_action must not error in phase 2");
    assert!(result.is_none());
}

#[test]
fn formatting_returns_none() {
    let backend = Workspace::new();
    let result = handlers::documents::prepare_formatting(
        &backend.snapshot(),
        DocumentFormattingParams {
            text_document: TextDocumentIdentifier {
                uri: pkl_url("a.pkl"),
            },
            options: FormattingOptions {
                tab_size: 2,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
        },
    )
    .and_then(|job| job.map(handlers::documents::formatting).transpose())
    .map(Option::flatten)
    .expect("formatting must not error in phase 2");
    assert!(result.is_none());
}
