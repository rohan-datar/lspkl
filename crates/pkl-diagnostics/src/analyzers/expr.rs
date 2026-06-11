//! Expression-level checks: always-true / always-false conditions, redundant
//! type casts, redundant null-safe access, etc.

use pkl_analysis::ast::{NodeId, NodeKind};
use pkl_types::compute_expr::compute_expr_type;
use pkl_types::to_type::to_type;
use pkl_types::{AnalysisContext, NodeRef, Type, TypeBindings};

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;

pub struct ExprAnalyzer;

impl Analyzer for ExprAnalyzer {
    fn analyze_node(
        &self,
        node: NodeId,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        match ctx.tree.kind(node) {
            NodeKind::TypeTestExpr => {
                let Some(expr_node) = ctx.tree.child(node, |c| c.is_expr_node()) else {
                    return true;
                };
                let Some(type_node) = ctx.tree.child(node, |c| c.is_type_node()) else {
                    return true;
                };
                let expr_type = compute_expr_type(expr_node, &TypeBindings::new(), ctx);
                let tested_type = to_type(
                    NodeRef {
                        file_id: ctx.file_id,
                        node_id: type_node,
                    },
                    &TypeBindings::new(),
                    ctx,
                );
                if !tested_type.constraints().is_empty() {
                    return false;
                }
                if tested_type.is_non_null(ctx.base) {
                    if !expr_type.is_nullable(ctx) || expr_type.is_non_null(ctx.base) {
                        holder.add_warning(ctx.tree.span(node), "expression is always true");
                    }
                } else {
                    if !matches!(expr_type, Type::Unknown)
                        && expr_type.is_subtype_of(&tested_type, ctx)
                    {
                        holder.add_warning(ctx.tree.span(node), "expression is always true");
                    } else if !tested_type.is_subtype_of(&expr_type, ctx) {
                        holder.add_warning(ctx.tree.span(node), "expression is always false");
                    }
                }
                false
            }
            NodeKind::TypeCastExpr => {
                let Some(expr_node) = ctx.tree.child(node, |c| c.is_expr_node()) else {
                    return true;
                };
                let Some(type_node) = ctx.tree.child(node, |c| c.is_type_node()) else {
                    return true;
                };
                let expr_type = compute_expr_type(expr_node, &TypeBindings::new(), ctx);
                let tested_type = to_type(
                    NodeRef {
                        file_id: ctx.file_id,
                        node_id: type_node,
                    },
                    &TypeBindings::new(),
                    ctx,
                );

                if tested_type.constraints().is_empty()
                    && !matches!(expr_type, Type::Unknown)
                    && expr_type.is_subtype_of(&tested_type, ctx)
                {
                    holder.add_error(ctx.tree.span(node), "type cast is redundant");
                    // TODO: add quickfix
                } else if !tested_type.is_subtype_of(&expr_type, ctx)
                    && !tested_type.has_common_subtype_with(&expr_type, ctx)
                {
                    holder.add_error(ctx.tree.span(node), "type cast cannot succeed");
                }

                false
            }
            NodeKind::NullCoalesceExpr => {
                let Some(expr_node) = ctx.tree.child(node, |c| c.is_expr_node()) else {
                    return true;
                };
                let expr_type = compute_expr_type(expr_node, &TypeBindings::new(), ctx);
                if !expr_type.is_nullable(ctx) {
                    holder.add_error(ctx.tree.span(node), "null coalescing is redundant");
                    // TODO: add quickfix
                }
                false
            }
            NodeKind::NonNullExpr => {
                let Some(expr_node) = ctx.tree.child(node, |c| c.is_expr_node()) else {
                    return true;
                };
                let expr_type = compute_expr_type(expr_node, &TypeBindings::new(), ctx);
                if !expr_type.is_nullable(ctx) {
                    holder.add_error(ctx.tree.span(node), "non null is redundant");
                    // TODO: add quickfix
                }
                false
            }
            _ => true,
        }
    }
}
