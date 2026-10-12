//! Surfaces parser errors and missing nodes as diagnostics.

use pkl_types::NodeRef;

use pkl_syntax::ast::NodeKind;
use pkl_types::AnalysisContext;

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;

pub struct SyntaxAnalyzer;

impl Analyzer for SyntaxAnalyzer {
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
        let node_kind = source_tree.kind(node);
        let span = source_tree.span(node);
        if matches!(node_kind, NodeKind::Error) {
            holder.add_error(span, "invalid syntax");
        } else if source_tree.is_missing(node) {
            holder.add_error(span, format!("missing syntax: {}", source_tree.text(node)));
        }

        true
    }
}
