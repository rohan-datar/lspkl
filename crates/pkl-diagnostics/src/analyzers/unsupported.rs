//! Flags Pkl features that aren't supported in the target Pkl version.

use pkl_analysis::ast::NodeId;
use pkl_types::AnalysisContext;

use crate::analyzer::Analyzer;
use crate::feature::{FEATURES, effective_pkl_version};
use crate::holder::DiagnosticsHolder;

pub struct UnsupportedFeatureAnalyzer;

impl Analyzer for UnsupportedFeatureAnalyzer {
    fn analyze_node(
        &self,
        node: NodeId,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        for feat in FEATURES.iter() {
            if (feat.predicate)(node, ctx) && effective_pkl_version(ctx) < feat.required_version {
                holder.add_error(ctx.tree.span(node), feat.message);
                return true;
            }
        }
        true
    }
}
