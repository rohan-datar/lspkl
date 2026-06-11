//! Validates type names

use pkl_analysis::ast::NodeId;
use pkl_types::{AnalysisContext, NodeRef};

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;

use pkl_types::resolver::resolve_type_name;

pub struct TypeNameAnalyzer;

impl Analyzer for TypeNameAnalyzer {
    fn analyze_node(
        &self,
        node: NodeId,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        if !ctx.tree.is_type_name(node) {
            return true;
        }

        let segments = ctx.tree.qualified_identifier_segments(node);
        if segments.is_empty() {
            return true;
        }

        if resolve_type_name(ctx, &segments, NodeRef::new(ctx.file_id, node)).is_some() {
            return true;
        }

        holder.add_error(
            ctx.tree.span(node),
            format!("unresolved reference: {}", segments.last().unwrap()),
        );

        false
    }
}
