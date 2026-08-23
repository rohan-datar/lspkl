//! GoToDefinition: resolve the node at the cursor and return its
//! declaration site (possibly in another file).
//!
//! Corresponds to
//! `pkl-lsp/src/main/kotlin/org/pkl/lsp/features/GoToDefinitionFeature.kt`.

use tower_lsp_server::lsp_types::{GotoDefinitionResponse, Location, Position, Uri};

use pkl_types::{AnalysisContext, NodeRef, resolver::resolve_reference};

use crate::span_to_range;

/// Resolve the definition for the symbol at `position`.
pub fn goto_definition(
    ctx: &AnalysisContext,
    position: Position,
    document_uri: &Uri,
) -> Option<GotoDefinitionResponse> {
    let line = position.line + 1;
    let col = position.character + 1;

    let node_id = ctx.tree.find_at(line, col)?;
    let original = NodeRef::new(ctx.file_id, node_id);
    let resolved = resolve_reference(original, line, col, ctx)?;

    if resolved.file_id != ctx.file_id {
        // The only reachable cross-file target today is pkl:base.
        // TODO(stdlib-goto): jumping into the stdlib needs the synthetic
        // `pkl-lsp://stdlib/` URI scheme plus content serving so the editor
        // can actually open it.
        return None;
    }

    let resolved_tree = ctx.tree_for(resolved.file_id)?;
    let resolved_span = resolved_tree.span(resolved.node_id);

    Some(GotoDefinitionResponse::Scalar(Location::new(
        document_uri.clone(),
        span_to_range(&resolved_span),
    )))
}
