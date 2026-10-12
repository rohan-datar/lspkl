//! Validates type names

use pkl_types::NodeRef;

use pkl_types::AnalysisContext;

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;

use pkl_types::resolver::resolve_type_name;

pub struct TypeNameAnalyzer;

impl Analyzer for TypeNameAnalyzer {
    fn analyze_node(
        &self,
        node: NodeRef,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        let file = node.file;
        let Some(source_tree) = ctx.tree(file) else {
            return false;
        };
        let node = node.value;
        if !source_tree.is_type_name(node) {
            return true;
        }

        let segments = source_tree.qualified_identifier_segments(node);
        if segments.is_empty() {
            return true;
        }

        if resolve_type_name(ctx, &segments, NodeRef::new(file, node)).is_some() {
            return true;
        }

        holder.add_error(
            source_tree.span(node),
            format!("unresolved reference: {}", segments.last().unwrap()),
        );

        false
    }
}
