//! `Analyzer` trait + the recursive walker that drives it.

use pkl_analysis::ast::NodeId;
use pkl_types::AnalysisContext;

use crate::holder::DiagnosticsHolder;

/// One pass over a `SyntaxTree`. Implementors examine `node`, optionally emit
/// diagnostics into `holder`, and return whether the walker should descend
/// into the node's children.
pub trait Analyzer: Send + Sync {
    /// Examine `node`. Return `true` to descend into its children, `false` to
    /// skip the subtree (e.g. for analyzers that handle their own recursion
    /// or short-circuit on certain shapes).
    fn analyze_node(
        &self,
        node: NodeId,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool;

    /// Stable name for logging and snapshot ordering. Default: type name.
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

/// Drive `analyzer` recursively over the subtree rooted at `node`.
pub fn run_analyzer(
    analyzer: &dyn Analyzer,
    node: NodeId,
    holder: &mut DiagnosticsHolder,
    ctx: &AnalysisContext,
) {
    if analyzer.analyze_node(node, holder, ctx) {
        for &child in ctx.tree.children(node) {
            run_analyzer(analyzer, child, holder, ctx);
        }
    }
}
