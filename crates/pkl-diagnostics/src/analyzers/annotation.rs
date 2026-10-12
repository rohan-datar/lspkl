//! Validates `@Annotation` usage

use pkl_types::NodeRef;

use pkl_syntax::ast::{NodeKind, nodes};
use pkl_types::{AnalysisContext, Type, resolver};

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;

pub struct AnnotationAnalyzer;

impl Analyzer for AnnotationAnalyzer {
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
        if !matches!(source_tree.kind(node), NodeKind::Annotation) {
            return true;
        }

        let Some(name_node) = nodes::Annotation::cast(source_tree, node)
            .and_then(|annotation| annotation.name(source_tree))
        else {
            return true;
        };
        let name_parts = source_tree.qualified_identifier_segments(name_node);
        let Some(class_node_ref) =
            resolver::resolve_type_name(ctx, &name_parts, NodeRef::new(file, name_node))
        else {
            holder.add_error(
                source_tree.span(node),
                format!("could not find annotation: {}", name_parts.join(".")),
            );
            return false;
        };

        let class_type = Type::class(class_node_ref);

        if !class_type.is_subtype_of(ctx.base.annotation_type(), ctx) {
            holder.add_error(source_tree.span(node), "invalid annotation");
        }
        false
    }
}
