//! `LanguageServer` trait implementation.
//!
//!
//! ## Why `client: Option<Client>`?
//!
//! `tower_lsp_server::LspService::new` hands the backend a `Client` for
//! sending notifications back to the editor (publish_diagnostics, etc.).
//! Tests construct `PklBackend` directly without that machinery, so the field
//! is optional.

use std::sync::Arc;

use tower_lsp_server::lsp_types::*;
use tower_lsp_server::{Client, LanguageServer, jsonrpc};

use fluent_uri::Uri;
use tracing::{error, info};

use crate::state::ServerState;

#[derive(Debug)]
pub struct PklBackend {
    pub state: Arc<ServerState>,
    /// `None` in tests, `Some` when wired up by `LspService::new`.
    pub client: Option<Client>,
}

impl PklBackend {
    pub fn new(state: Arc<ServerState>, client: Option<Client>) -> Self {
        Self { state, client }
    }

    /// Helper for tests: a backend with no client wired up.
    pub fn for_testing() -> Self {
        Self {
            state: Arc::new(ServerState::new()),
            client: None,
        }
    }
}

impl LanguageServer for PklBackend {
    async fn initialize(&self, params: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        info!(?params.client_info, "initilize");
        Ok(InitializeResult {
            server_info: None,
            capabilities: ServerCapabilities {
                position_encoding: None,
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
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
                code_action_provider: Some(CodeActionProviderCapability::Options(
                    CodeActionOptions {
                        code_action_kinds: Some(vec![CodeActionKind::QUICKFIX]),
                        work_done_progress_options: WorkDoneProgressOptions {
                            work_done_progress: None,
                        },
                        resolve_provider: None,
                    },
                )),
                code_lens_provider: None,
                document_formatting_provider: Some(OneOf::Left(true)),
                document_range_formatting_provider: None,
                document_on_type_formatting_provider: None,
                rename_provider: None,
                document_link_provider: None,
                color_provider: None,
                folding_range_provider: None,
                declaration_provider: None,
                execute_command_provider: None,
                workspace: None,
                call_hierarchy_provider: None,
                semantic_tokens_provider: None,
                moniker_provider: None,
                linked_editing_range_provider: None,
                inline_value_provider: None,
                inlay_hint_provider: None,
                diagnostic_provider: None,
                experimental: None,
            },
        })
    }

    async fn initialized(&self, params: InitializedParams) {
        let _ = params;
        info!("server intialized")
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        info!("server shutdown");
        Ok(())
    }

    // ---- Text document lifecycle (Phase 2 core) ----

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        info!(uri = %params.text_document.uri.as_str(), "didOpen");
        let Ok(url) = Uri::parse(params.text_document.uri.as_str().to_string()) else {
            error!(uri = %params.text_document.uri.as_str(), "Failed to parse URI!");
            return;
        };

        self.state.file_manager.open(
            url,
            params.text_document.text,
            Some(params.text_document.version),
        );
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        info!(uri = %params.text_document.uri.as_str(), "didChange");
        let Ok(url) = Uri::parse(params.text_document.uri.as_str().to_string()) else {
            error!(uri = %params.text_document.uri.as_str(), "Failed to parse URI!");
            return;
        };

        match self.state.file_manager.update(
            &url,
            params.content_changes[0].text.clone(),
            Some(params.text_document.version),
        ) {
            Ok(_) => (),
            Err(err) => {
                error!(uri = %params.text_document.uri.as_str(), url = %url.clone(), err = ?err, "update failed!")
            }
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let _ = params;
        // No-op for now. Later phases may trigger project/package refresh here.
        todo!("save handler — no-op for now")
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        info!(uri = %params.text_document.uri.as_str(), "didClose");
        let Ok(url) = Uri::parse(params.text_document.uri.as_str().to_string()) else {
            error!(uri = %params.text_document.uri.as_str(), "Failed to parse URI!");
            return;
        };

        self.state.file_manager.close(&url);
    }

    // ---- LSP features (stubs until Phase 5) ----

    async fn hover(&self, params: HoverParams) -> jsonrpc::Result<Option<Hover>> {
        let _ = params;
        // TODO: port HoverFeature.kt. Until then, return None — clients
        // expect that as "no hover info available", not as an error.
        Ok(None)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> jsonrpc::Result<Option<GotoDefinitionResponse>> {
        let _ = params;
        // TODO: port GoToDefinitionFeature.kt.
        Ok(None)
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> jsonrpc::Result<Option<CompletionResponse>> {
        let _ = params;
        // TODO: port CompletionFeature.kt.
        Ok(None)
    }

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> jsonrpc::Result<Option<CodeActionResponse>> {
        let _ = params;
        // TODO: port CodeActionFeature.kt.
        Ok(None)
    }

    async fn formatting(
        &self,
        params: DocumentFormattingParams,
    ) -> jsonrpc::Result<Option<Vec<TextEdit>>> {
        let _ = params;
        // TODO: shell out to `pkl format`.
        Ok(None)
    }
}
