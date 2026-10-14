//! Validates class/object members — duplicate-name checks, override
//! correctness, abstract members in concrete classes, etc.

use lspkl_types::NodeRef;

use lspkl_syntax::ast::{NodeKind, nodes};
use lspkl_types::{AnalysisContext, Type};

use lspkl_types::find_member;

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;

pub struct MemberAnalyzer;

impl Analyzer for MemberAnalyzer {
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
        if !matches!(
            source_tree.kind(node),
            NodeKind::ClassProperty | NodeKind::ObjectProperty
        ) {
            return true;
        }

        // Defining (has TypeAnnotation child) — never flag.
        if nodes::ClassProperty::cast(source_tree, node)
            .and_then(|property| property.type_annotation(source_tree))
            .or_else(|| {
                nodes::ObjectProperty::cast(source_tree, node)
                    .and_then(|property| property.type_annotation(source_tree))
            })
            .is_some()
        {
            return true;
        }

        let name = source_tree.name(node).unwrap_or("");
        if name.is_empty() {
            return true;
        }

        let this_type = lspkl_types::this_type(ctx.db, NodeRef::new(file, node), ctx.root);

        // Unknown, Dynamic, and non-Object receivers have no analyzable members.
        if matches!(this_type, Type::Unknown) {
            return true;
        }
        if this_type.class_equals(ctx.base.dynamic_type()) {
            return true;
        }
        if !this_type.is_subtype_of(ctx.base.object_type(), ctx.db) {
            return true;
        }

        // Look up the property name on the this-type. If found → OK; if not → flag.
        if find_member(ctx.db, &this_type, name).is_some() {
            return true;
        }

        let ident_span = source_tree
            .identifier(node)
            .map(|i| source_tree.span(i))
            .unwrap_or_else(|| source_tree.span(node));
        holder.add_warning(ident_span, format!("unresolved property: {name}"));
        true
    }
}
