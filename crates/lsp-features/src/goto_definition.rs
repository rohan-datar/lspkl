//! GoToDefinition: resolve the node at the cursor and return its
//! declaration site (possibly in another file).

use lsp_types::{GotoDefinitionResponse, Location, Range, Uri};

use pkl_types::{AnalysisContext, FileId, NodeRef, resolver::resolve_reference};

use pkl_syntax::{TextRange, TextSize, ast::SyntaxTree};

/// Resolve the definition for the symbol at byte `offset`.
pub fn goto_definition(
    ctx: &AnalysisContext,
    offset: TextSize,
    uri_for: impl Fn(FileId) -> Option<Uri>,
    range_for: impl Fn(&SyntaxTree, TextRange) -> Range,
) -> Option<GotoDefinitionResponse> {
    let node_id = ctx.tree.find_at(offset)?;
    let mut current = Some(node_id);
    let mut in_string = false;
    while let Some(id) = current {
        in_string |= ctx.tree.kind(id).is_string_literal();
        if in_string
            && let Some(pkl_types::ImportResolution::Glob(targets)) =
                ctx.imports.get(&NodeRef::new(ctx.file_id, id))
        {
            let locations: Vec<_> = targets
                .iter()
                .filter_map(|target| {
                    let tree = ctx.tree_for(target.file_id)?;
                    Some(Location::new(
                        uri_for(target.file_id)?,
                        range_for(tree, tree.span(target.node_id)),
                    ))
                })
                .collect();
            return (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations));
        }
        current = ctx.tree.parent(id);
    }
    let original = NodeRef::new(ctx.file_id, node_id);
    let resolved = resolve_reference(original, offset, ctx)?;

    let uri = uri_for(resolved.file_id)?;
    let resolved_tree = ctx.tree_for(resolved.file_id)?;
    let resolved_span = resolved_tree.span(resolved.node_id);

    Some(GotoDefinitionResponse::Scalar(Location::new(
        uri,
        range_for(resolved_tree, resolved_span),
    )))
}
