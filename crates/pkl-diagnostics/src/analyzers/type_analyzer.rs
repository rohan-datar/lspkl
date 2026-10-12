//! Validates type expressions

use pkl_types::NodeRef;

use pkl_syntax::ast::nodes;

use pkl_types::{AnalysisContext, Type};

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;

pub struct TypeAnalyzer;

impl Analyzer for TypeAnalyzer {
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
        let Some(declared) = nodes::DeclaredType::cast(source_tree, node) else {
            return true;
        };
        let arg_count = declared.arguments(source_tree).count();
        if arg_count == 0 {
            return true;
        }
        let node_type = NodeRef::new(file, node)
            .cast(ctx, pkl_syntax::ast::nodes::TypeNode::cast)
            .map_or(Type::Unknown, |ty| pkl_types::to_type(ty, ctx));

        let param_count = match node_type.unconstrained() {
            Type::Class { type_args, .. } => type_args.len(),
            Type::Alias { type_args, .. } => type_args.len(),
            _ => 0,
        };
        if param_count != 0 && param_count != arg_count {
            holder.add_error(
                source_tree.span(node),
                format!(
                    "incorrect number of type arguments. Expected: {param_count}, Got: {arg_count}"
                ),
            );
        }
        false
    }
}
