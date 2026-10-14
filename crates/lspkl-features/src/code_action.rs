//! CodeAction: turn diagnostics-attached quick fixes into LSP `CodeAction`s.

use std::collections::HashMap;

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, Command, Range, TextEdit,
    WorkspaceEdit,
};

use lspkl_diagnostics::Diagnostic;
use lspkl_diagnostics::diagnostic::QuickFixCommand;
use lspkl_syntax::TextRange;

/// Materialize quick-fix actions for the diagnostics echoed in
/// `params.context.diagnostics`.
pub fn code_action(
    params: &CodeActionParams,
    diagnostics: &[Diagnostic],
    range_for: impl Fn(TextRange) -> Range,
) -> Vec<CodeActionOrCommand> {
    let uri = params.text_document.uri.clone();
    let mut actions = Vec::new();

    for echoed in &params.context.diagnostics {
        // Match on (range, message): LSP diagnostics carry no stable id, and the
        // client echoes both verbatim from `diagnostics::to_lsp`.
        let Some(matched) = diagnostics
            .iter()
            .find(|d| d.message == echoed.message && range_for(d.span) == echoed.range)
        else {
            continue;
        };

        for fix in &matched.quick_fixes {
            let edit = (!fix.edits.is_empty()).then(|| {
                // LSP URI keys have interior mutability, but the protocol requires this map.
                #[allow(clippy::mutable_key_type)]
                let mut changes = HashMap::new();
                changes.insert(
                    uri.clone(),
                    fix.edits
                        .iter()
                        .map(|e| TextEdit {
                            range: range_for(e.span),
                            new_text: e.new_text.clone(),
                        })
                        .collect(),
                );
                WorkspaceEdit {
                    changes: Some(changes),
                    ..Default::default()
                }
            });

            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: fix.title.clone(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![echoed.clone()]),
                is_preferred: Some(true),
                edit,
                command: fix.command.as_ref().map(|command| match command {
                    QuickFixCommand::DownloadPackage { uri } => Command {
                        title: fix.title.clone(),
                        command: "pkl.downloadPackage".into(),
                        arguments: Some(vec![uri.clone().into()]),
                    },
                }),
                ..Default::default()
            }));
        }
    }

    actions
}
