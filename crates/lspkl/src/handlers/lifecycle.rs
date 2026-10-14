//! Initialization, workspace notifications, and restored project metadata.

use super::{HandlerError, Result};
use crate::{
    perf,
    state::{ReloadWork, Snapshot, Workspace},
};
use lsp_types::*;
use lspkl_db::settings::WorkspaceSettings;
use lspkl_features::positions::PositionEncoding;
use tracing::error;

pub fn initialize(workspace: &mut Workspace, params: &InitializeParams) -> InitializeResult {
    workspace.apply_initialize(params);
    let encoding = workspace.snapshot().position_encoding;
    InitializeResult {
        server_info: None,
        capabilities: ServerCapabilities {
            position_encoding: Some(match encoding {
                PositionEncoding::Utf8 => PositionEncodingKind::UTF8,
                PositionEncoding::Utf16 => PositionEncodingKind::UTF16,
            }),
            text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
            notebook_document_sync: None,
            selection_range_provider: None,
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            completion_provider: Some(CompletionOptions {
                resolve_provider: Some(true),
                trigger_characters: Some(
                    [".", "/", "\"", ":"]
                        .into_iter()
                        .map(str::to_string)
                        .chain(('a'..='z').map(|c| c.to_string()))
                        .chain(('A'..='Z').map(|c| c.to_string()))
                        .collect(),
                ),
                all_commit_characters: None,
                work_done_progress_options: WorkDoneProgressOptions {
                    work_done_progress: None,
                },
                completion_item: None,
            }),
            signature_help_provider: None,
            definition_provider: Some(OneOf::Left(true)),
            type_definition_provider: None,
            implementation_provider: None,
            references_provider: None,
            document_highlight_provider: None,
            document_symbol_provider: None,
            workspace_symbol_provider: None,
            code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
                code_action_kinds: Some(vec![CodeActionKind::QUICKFIX]),
                work_done_progress_options: WorkDoneProgressOptions {
                    work_done_progress: None,
                },
                resolve_provider: None,
            })),
            code_lens_provider: None,
            document_formatting_provider: Some(OneOf::Left(true)),
            document_range_formatting_provider: None,
            document_on_type_formatting_provider: None,
            rename_provider: None,
            document_link_provider: None,
            color_provider: None,
            folding_range_provider: None,
            declaration_provider: None,
            execute_command_provider: Some(ExecuteCommandOptions {
                commands: vec!["pkl.downloadPackage".into(), "pkl.syncProjects".into()],
                work_done_progress_options: Default::default(),
            }),
            workspace: Some(WorkspaceServerCapabilities {
                workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                    supported: Some(true),
                    change_notifications: Some(OneOf::Left(true)),
                }),
                file_operations: None,
            }),
            call_hierarchy_provider: None,
            semantic_tokens_provider: None,
            moniker_provider: None,
            linked_editing_range_provider: None,
            inline_value_provider: None,
            inlay_hint_provider: None,
            diagnostic_provider: None,
            experimental: None,
        },
    }
}
pub fn did_open(workspace: &mut Workspace, params: DidOpenTextDocumentParams) -> Result<()> {
    workspace.apply_open(params)?;
    Ok(())
}
pub fn did_change(workspace: &mut Workspace, params: DidChangeTextDocumentParams) -> Result<()> {
    if let Some(stored_version) = workspace
        .document(params.text_document.uri.as_str())
        .and_then(|file| file.version(&workspace.db))
        && params.text_document.version <= stored_version
    {
        let ordering_violations = perf::ordering_violation();
        tracing::warn!(target: "lspkl::perf", uri = params.text_document.uri.as_str(), incoming_version = params.text_document.version, stored_version, ordering_violations, "non-increasing didChange version; applying incoming text");
    }
    workspace.apply_change(params)?;
    Ok(())
}
pub fn did_close(
    workspace: &mut Workspace,
    params: DidCloseTextDocumentParams,
) -> Result<ReloadWork> {
    Ok(workspace.apply_close(params)?)
}
pub fn did_change_configuration(
    workspace: &mut Workspace,
    params: DidChangeConfigurationParams,
) -> Result<()> {
    let settings = WorkspaceSettings::parse(params.settings)
        .map_err(|err| HandlerError::InvalidParams(err.to_string()))?;
    workspace.apply_explicit_settings(settings);
    Ok(())
}
pub fn did_change_workspace_folders(
    workspace: &mut Workspace,
    params: DidChangeWorkspaceFoldersParams,
) {
    workspace.apply_workspace_folders(params);
}

pub fn prepare_restore_workspaces(
    snapshot: &Snapshot,
) -> lspkl_project::project_manager::ProjectManager {
    snapshot.project_manager.lock().clone()
}
pub fn restore_workspaces(
    mut manager: lspkl_project::project_manager::ProjectManager,
) -> Result<lspkl_project::project_manager::ProjectManager> {
    for root in manager.workspace_folders().to_vec() {
        let path = root.join(".lspkl/projects.json");
        match path.try_exists() {
            Ok(true) => {
                if let Err(err) = manager.load_state(&path) {
                    error!(path = %path.display(), ?err, "could not restore project state");
                }
            }
            Ok(false) => {}
            Err(err) => error!(path = %path.display(), ?err, "could not inspect project state"),
        }
    }
    Ok(manager)
}
