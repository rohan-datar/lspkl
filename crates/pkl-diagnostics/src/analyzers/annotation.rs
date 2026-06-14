//! Validates `@Annotation` usage

use pkl_syntax::ast::{NodeId, NodeKind};
use pkl_types::{AnalysisContext, NodeRef, Type, resolver};

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;

pub struct AnnotationAnalyzer;

impl Analyzer for AnnotationAnalyzer {
    fn analyze_node(
        &self,
        node: NodeId,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        if !matches!(ctx.tree.kind(node), NodeKind::Annotation) {
            return true;
        }

        let Some(name_node) = ctx
            .tree
            .child(node, |c| matches!(c, NodeKind::QualifiedIdentifier))
        else {
            return true;
        };
        let name_parts = ctx.tree.qualified_identifier_segments(name_node);
        let Some(class_node_ref) =
            resolver::resolve_type_name(ctx, &name_parts, NodeRef::new(ctx.file_id, name_node))
        else {
            holder.add_error(
                ctx.tree.span(node),
                format!("could not find annotation: {}", name_parts.join(".")),
            );
            return false;
        };

        let class_type = Type::class(class_node_ref);

        if !class_type.is_subtype_of(ctx.base.annotation_type(), ctx) {
            holder.add_error(ctx.tree.span(node), "invalid annotation");
        }
        false
    }
}
