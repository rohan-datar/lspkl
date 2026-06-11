//! Validates type expressions

use pkl_analysis::ast::{NodeId, NodeKind};
use pkl_types::to_type::to_type;
use pkl_types::{AnalysisContext, NodeRef, Type, TypeBindings};

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;

pub struct TypeAnalyzer;

impl Analyzer for TypeAnalyzer {
    fn analyze_node(
        &self,
        node: NodeId,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        if !matches!(ctx.tree.kind(node), NodeKind::DeclaredType)
            || ctx
                .tree
                .child(node, |k| matches!(k, NodeKind::TypeArgumentList))
                .is_none()
        {
            return true;
        }

        let node_type = to_type(NodeRef::new(ctx.file_id, node), &TypeBindings::new(), ctx);
        let type_arg_list = ctx
            .tree
            .child(node, |k| matches!(k, NodeKind::TypeArgumentList))
            .unwrap();
        let arg_count = ctx.tree.children(type_arg_list).len();

        let param_count = match node_type {
            Type::Class { type_args, .. } => type_args.len(),
            Type::Alias { type_args, .. } => type_args.len(),
            _ => 0,
        };
        if param_count != 0 && param_count != arg_count {
            holder.add_error(
                ctx.tree.span(node),
                format!(
                    "incorrect number of type arguments. Expected: {param_count}, Got: {arg_count}"
                ),
            );
        }
        false
    }
}
