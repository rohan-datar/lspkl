//! Plain handlers exercise semantics without a runtime or transport client.
use lsp_types::*;
use lspkl::{Snapshot, Workspace, handlers};

fn open(workspace: &mut Workspace, uri: &Uri, text: &str) {
    handlers::lifecycle::did_open(
        workspace,
        DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "pkl".into(),
                version: 1,
                text: text.into(),
            },
        },
    )
    .unwrap();
}
fn point(uri: &Uri, line: u32, character: u32) -> TextDocumentPositionParams {
    TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        position: Position::new(line, character),
    }
}
fn hover(snapshot: &Snapshot, uri: &Uri) -> Hover {
    handlers::documents::hover(
        snapshot,
        HoverParams {
            text_document_position_params: point(uri, 1, 4),
            work_done_progress_params: Default::default(),
        },
    )
    .unwrap()
    .unwrap()
}
#[test]
fn semantic_handlers_use_the_captured_buffer_epoch() {
    let mut workspace = Workspace::new();
    let uri: Uri = "inmemory:///handler.pkl".parse().unwrap();
    open(&mut workspace, &uri, "x: Int = 1\ny = x\n");
    let before = workspace.snapshot();
    handlers::lifecycle::did_change(
        &mut workspace,
        DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "x: String = \"edited\"\ny = x\n".into(),
            }],
        },
    )
    .unwrap();
    let after = workspace.snapshot();
    let HoverContents::Markup(before_hover) = hover(&before, &uri).contents else {
        panic!("expected markup")
    };
    let HoverContents::Markup(after_hover) = hover(&after, &uri).contents else {
        panic!("expected markup")
    };
    assert!(
        before_hover.value.contains("x: Int"),
        "{}",
        before_hover.value
    );
    assert!(
        after_hover.value.contains("x: String"),
        "{}",
        after_hover.value
    );
    assert_eq!(
        handlers::documents::file_contents(&before, TextDocumentIdentifier { uri: uri.clone() })
            .unwrap(),
        "x: Int = 1\ny = x\n"
    );
    let definition = handlers::documents::definition(
        &before,
        GotoDefinitionParams {
            text_document_position_params: point(&uri, 1, 4),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        },
    )
    .unwrap()
    .unwrap();
    let GotoDefinitionResponse::Scalar(location) = definition else {
        panic!("expected one definition")
    };
    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(0, 0));
    let completion = handlers::documents::completion(
        &before,
        CompletionParams {
            text_document_position: point(&uri, 1, 5),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        },
    )
    .unwrap()
    .unwrap();
    let items = match completion {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(list) => list.items,
    };
    assert!(items.iter().any(|item| item.label == "x"));
    handlers::lifecycle::did_close(
        &mut workspace,
        DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
        },
    )
    .unwrap();
    assert!(workspace.snapshot().document(uri.as_str()).is_none());
    let HoverContents::Markup(closed_hover) = hover(&before, &uri).contents else {
        panic!("expected markup")
    };
    assert!(
        closed_hover.value.contains("x: Int"),
        "{}",
        closed_hover.value
    );
}
#[test]
fn diagnostics_and_quickfix_share_actual_analysis() {
    let mut workspace = Workspace::new();
    let uri: Uri = "inmemory:///fix.pkl".parse().unwrap();
    open(
        &mut workspace,
        &uri,
        "foo = new Dynamic {\n  const bar = 1\n}\n",
    );
    let snapshot = workspace.snapshot();
    let diagnostics = handlers::diagnostics::diagnostics(&snapshot, uri.as_str()).unwrap();
    let actions = handlers::documents::code_action(
        &snapshot,
        CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range::default(),
            context: CodeActionContext {
                diagnostics,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        },
    )
    .unwrap()
    .unwrap();
    let edits = actions
        .iter()
        .find_map(|action| match action {
            CodeActionOrCommand::CodeAction(action) => {
                action.edit.as_ref()?.changes.as_ref()?.get(&uri)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        edits,
        &[TextEdit::new(
            Range::new(Position::new(1, 2), Position::new(1, 2)),
            "local ".into()
        )]
    );
    handlers::lifecycle::did_change(
        &mut workspace,
        DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "foo = new Dynamic {\n  local const bar = 1\n}\n".into(),
            }],
        },
    )
    .unwrap();
    assert!(
        handlers::diagnostics::diagnostics(&workspace.snapshot(), uri.as_str())
            .unwrap()
            .is_empty()
    );
}
#[test]
fn command_errors_have_one_protocol_mapping() {
    let snapshot = Workspace::new().snapshot();
    for params in [
        ExecuteCommandParams {
            command: "unknown".into(),
            arguments: vec![],
            work_done_progress_params: Default::default(),
        },
        ExecuteCommandParams {
            command: "pkl.downloadPackage".into(),
            arguments: vec![42.into()],
            work_done_progress_params: Default::default(),
        },
    ] {
        let error: tower_lsp_server::jsonrpc::Error =
            handlers::commands::execute_command(&snapshot, params)
                .unwrap_err()
                .into();
        assert_eq!(
            error.code,
            tower_lsp_server::jsonrpc::ErrorCode::InvalidParams
        );
    }
    let error: tower_lsp_server::jsonrpc::Error =
        handlers::commands::download_package(&snapshot, "not-a-package".into())
            .unwrap_err()
            .into();
    assert_eq!(
        error.code,
        tower_lsp_server::jsonrpc::ErrorCode::InvalidParams
    );
}

#[cfg(unix)]
#[test]
fn formatting_preserves_cli_exit_codes_and_captured_source() {
    use std::{fs, os::unix::fs::PermissionsExt};
    let temp = tempfile::tempdir().unwrap();
    let cli = temp.path().join("pkl");
    fs::write(
        &cli,
        "#!/bin/sh
[ \"$1\" = format ] || exit 9
[ \"$2\" = - ] || exit 9
input=$(cat)
case \"$input\" in
  unchanged) exit 0 ;;
  fail) echo 'fixture formatter failure' >&2; exit 7 ;;
  *) printf 'x = 1\\n'; exit 11 ;;
esac
",
    )
    .unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    let mut workspace = Workspace::new();
    workspace.apply_settings(
        lspkl::settings::WorkspaceSettings::parse(serde_json::json!({"cli": {"path": cli}}))
            .unwrap(),
    );
    let uri: Uri = "inmemory:///format.pkl".parse().unwrap();
    open(&mut workspace, &uri, "x=1");
    let before = workspace.snapshot();
    let params = || DocumentFormattingParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        options: FormattingOptions {
            tab_size: 2,
            insert_spaces: true,
            ..Default::default()
        },
        work_done_progress_params: Default::default(),
    };
    assert_eq!(
        handlers::documents::formatting(&before, params())
            .unwrap()
            .unwrap()[0]
            .new_text,
        "x = 1\n"
    );
    for source in ["unchanged", "fail"] {
        handlers::lifecycle::did_change(
            &mut workspace,
            DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version: 2,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: source.into(),
                }],
            },
        )
        .unwrap();
        let result = handlers::documents::formatting(&workspace.snapshot(), params()).unwrap();
        if source == "unchanged" {
            assert!(result.unwrap().is_empty());
        } else {
            assert!(result.is_none());
        }
    }
    assert_eq!(
        handlers::documents::formatting(&before, params())
            .unwrap()
            .unwrap()[0]
            .new_text,
        "x = 1\n"
    );
}
