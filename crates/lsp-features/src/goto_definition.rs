//! GoToDefinition: resolve the node at the cursor and return its
//! declaration site (possibly in another file).

use lsp_types::{GotoDefinitionResponse, Location, Range, Uri};

use pkl_types::{AnalysisContext, FileId, NodeRef, resolve_reference};

use pkl_syntax::{TextRange, TextSize, ast::SyntaxTree};

/// Resolve the definition for the symbol at the file-qualified byte `offset`.
pub fn goto_definition(
    ctx: &AnalysisContext,
    offset: pkl_types::InFile<TextSize>,
    uri_for: impl Fn(FileId) -> Option<Uri>,
    range_for: impl Fn(&SyntaxTree, TextRange) -> Range,
) -> Option<GotoDefinitionResponse> {
    let file = offset.file;
    let tree = ctx.tree(file)?;
    let offset = offset.value;
    let node_id = tree.find_at(offset)?;
    let mut current = Some(node_id);
    let mut in_string = false;
    while let Some(id) = current {
        in_string |= tree.kind(id).is_string_literal();
        if in_string
            && let Some(pkl_types::ImportResolution::Glob(targets)) =
                ctx.imports.get(&NodeRef::new(file, id))
        {
            let locations: Vec<_> = targets
                .iter()
                .filter_map(|target| {
                    let tree = ctx.tree(target.file)?;
                    Some(Location::new(
                        uri_for(target.file)?,
                        range_for(tree, tree.span(target.value)),
                    ))
                })
                .collect();
            return (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations));
        }
        current = tree.parent(id);
    }
    let original = NodeRef::new(file, node_id);
    let resolved = resolve_reference(original, offset, ctx)?;

    let uri = uri_for(resolved.file)?;
    let resolved_tree = ctx.tree(resolved.file)?;
    let resolved_span = resolved_tree.span(resolved.value);

    Some(GotoDefinitionResponse::Scalar(Location::new(
        uri,
        range_for(resolved_tree, resolved_span),
    )))
}
