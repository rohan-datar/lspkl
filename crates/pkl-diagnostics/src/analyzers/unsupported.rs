//! Flags Pkl features that aren't supported in the target Pkl version.

use pkl_types::NodeRef;

use pkl_types::AnalysisContext;

use crate::analyzer::Analyzer;
use crate::feature::{FEATURES, effective_pkl_version};
use crate::holder::DiagnosticsHolder;

pub struct UnsupportedFeatureAnalyzer;

impl Analyzer for UnsupportedFeatureAnalyzer {
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
        for feat in FEATURES.iter() {
            if (feat.predicate)(NodeRef::new(file, node), ctx)
                && effective_pkl_version(ctx, file) < feat.required_version
            {
                holder.add_error(source_tree.span(node), feat.message);
                return true;
            }
        }
        true
    }
}
