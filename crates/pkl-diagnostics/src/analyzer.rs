//! `Analyzer` trait + the recursive walker that drives it.

use pkl_syntax::ast::SyntaxTree;
use pkl_types::{AnalysisContext, NodeRef};

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
        node: NodeRef,
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
    node: NodeRef,
    holder: &mut DiagnosticsHolder,
    ctx: &AnalysisContext,
) {
    let Some(tree) = ctx.tree(node.file) else {
        return;
    };
    run_analyzer_with_tree(analyzer, node, holder, ctx, tree);
}

/// Walk a same-file subtree using the tree already captured by the engine.
pub(crate) fn run_analyzer_with_tree(
    analyzer: &dyn Analyzer,
    node: NodeRef,
    holder: &mut DiagnosticsHolder,
    ctx: &AnalysisContext,
    tree: &SyntaxTree,
) {
    if analyzer.analyze_node(node, holder, ctx) {
        for &child in tree.children(node.value) {
            run_analyzer_with_tree(analyzer, node.with_value(child), holder, ctx, tree);
        }
    }
}
