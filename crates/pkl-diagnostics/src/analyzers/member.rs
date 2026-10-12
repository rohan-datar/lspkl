//! Validates class/object members — duplicate-name checks, override
//! correctness, abstract members in concrete classes, etc.

use pkl_syntax::ast::{NodeId, NodeKind, nodes};
use pkl_types::{AnalysisContext, Type, TypeBindings};

use pkl_types::compute_expr::compute_this_type;

use pkl_types::resolver::find_member_on_type;

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;

pub struct MemberAnalyzer;

impl Analyzer for MemberAnalyzer {
    fn analyze_node(
        &self,
        node: NodeId,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        if !matches!(
            ctx.tree.kind(node),
            NodeKind::ClassProperty | NodeKind::ObjectProperty
        ) {
            return true;
        }

        // Defining (has TypeAnnotation child) — never flag.
        if nodes::ClassProperty::cast(ctx.tree, node)
            .and_then(|property| property.type_annotation(ctx.tree))
            .or_else(|| {
                nodes::ObjectProperty::cast(ctx.tree, node)
                    .and_then(|property| property.type_annotation(ctx.tree))
            })
            .is_some()
        {
            return true;
        }

        let name = ctx.tree.name(node).unwrap_or("");
        if name.is_empty() {
            return true;
        }

        let bindings = TypeBindings::new();
        let this_type = compute_this_type(node, &bindings, ctx);

        // Unknown, Dynamic, and non-Object receivers have no analyzable members.
        if matches!(this_type, Type::Unknown) {
            return true;
        }
        if this_type.class_equals(ctx.base.dynamic_type()) {
            return true;
        }
        if !this_type.is_subtype_of(ctx.base.object_type(), ctx) {
            return true;
        }

        // Look up the property name on the this-type. If found → OK; if not → flag.
        if find_member_on_type(&this_type, name, &bindings, ctx).is_some() {
            return true;
        }

        let ident_span = ctx
            .tree
            .identifier(node)
            .map(|i| ctx.tree.span(i))
            .unwrap_or_else(|| ctx.tree.span(node));
        holder.add_warning(ident_span, format!("unresolved property: {name}"));
        true
    }
}
