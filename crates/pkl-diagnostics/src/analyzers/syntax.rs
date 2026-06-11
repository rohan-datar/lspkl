//! Surfaces parser errors and missing nodes as diagnostics.

use pkl_analysis::ast::{NodeId, NodeKind};
use pkl_types::AnalysisContext;

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;

pub struct SyntaxAnalyzer;

impl Analyzer for SyntaxAnalyzer {
    fn analyze_node(
        &self,
        node: NodeId,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        let node_kind = ctx.tree.kind(node);
        let span = ctx.tree.span(node);
        if matches!(node_kind, NodeKind::Error) {
            holder.add_error(span, "invalid syntax");
        } else if ctx.tree.is_missing(node) {
            holder.add_error(span, format!("missing syntax: {}", ctx.tree.text(node)));
        }

        true
    }
}
