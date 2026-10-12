//! Expression-level checks: always-true / always-false conditions, redundant
//! type casts, redundant null-safe access, etc.

use pkl_types::NodeRef;

use pkl_syntax::ast::{NodeKind, nodes};

use pkl_types::{AnalysisContext, Type};

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;

pub struct ExprAnalyzer;

impl Analyzer for ExprAnalyzer {
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
        match source_tree.kind(node) {
            NodeKind::TypeTestExpr => {
                let Some(expr_node) = nodes::BinaryExpr::cast(source_tree, node)
                    .and_then(|expr| expr.left(source_tree))
                else {
                    return true;
                };
                let Some(type_node) = nodes::BinaryExpr::cast(source_tree, node)
                    .and_then(|expr| expr.right(source_tree))
                else {
                    return true;
                };
                let expr_type = NodeRef::new(file, expr_node)
                    .cast(ctx, pkl_syntax::ast::nodes::Expr::cast)
                    .map_or(Type::Unknown, |expr| pkl_types::expr_type(expr, ctx));
                let tested_type = NodeRef::new(file, type_node)
                    .cast(ctx, pkl_syntax::ast::nodes::TypeNode::cast)
                    .map_or(Type::Unknown, |ty| pkl_types::to_type(ty, ctx));
                if !tested_type.constraints().is_empty() {
                    return false;
                }
                if tested_type.is_non_null(ctx.base) {
                    if !expr_type.is_nullable(ctx) || expr_type.is_non_null(ctx.base) {
                        holder.add_warning(source_tree.span(node), "expression is always true");
                    }
                } else {
                    if !matches!(expr_type, Type::Unknown)
                        && expr_type.is_subtype_of(&tested_type, ctx)
                    {
                        holder.add_warning(source_tree.span(node), "expression is always true");
                    } else if !tested_type.is_subtype_of(&expr_type, ctx) {
                        holder.add_warning(source_tree.span(node), "expression is always false");
                    }
                }
                false
            }
            NodeKind::TypeCastExpr => {
                let Some(expr_node) = nodes::BinaryExpr::cast(source_tree, node)
                    .and_then(|expr| expr.left(source_tree))
                else {
                    return true;
                };
                let Some(type_node) = nodes::BinaryExpr::cast(source_tree, node)
                    .and_then(|expr| expr.right(source_tree))
                else {
                    return true;
                };
                let expr_type = NodeRef::new(file, expr_node)
                    .cast(ctx, pkl_syntax::ast::nodes::Expr::cast)
                    .map_or(Type::Unknown, |expr| pkl_types::expr_type(expr, ctx));
                let tested_type = NodeRef::new(file, type_node)
                    .cast(ctx, pkl_syntax::ast::nodes::TypeNode::cast)
                    .map_or(Type::Unknown, |ty| pkl_types::to_type(ty, ctx));

                if tested_type.constraints().is_empty()
                    && !matches!(expr_type, Type::Unknown)
                    && expr_type.is_subtype_of(&tested_type, ctx)
                {
                    holder.add_error(source_tree.span(node), "type cast is redundant");
                    // TODO: add quickfix
                } else if !tested_type.is_subtype_of(&expr_type, ctx)
                    && !tested_type.has_common_subtype_with(&expr_type, ctx)
                {
                    holder.add_error(source_tree.span(node), "type cast cannot succeed");
                }

                false
            }
            NodeKind::NullCoalesceExpr => {
                let Some(expr_node) = nodes::BinaryExpr::cast(source_tree, node)
                    .and_then(|expr| expr.left(source_tree))
                else {
                    return true;
                };
                let expr_type = NodeRef::new(file, expr_node)
                    .cast(ctx, pkl_syntax::ast::nodes::Expr::cast)
                    .map_or(Type::Unknown, |expr| pkl_types::expr_type(expr, ctx));
                if !expr_type.is_nullable(ctx) {
                    holder.add_error(source_tree.span(node), "null coalescing is redundant");
                    // TODO: add quickfix
                }
                false
            }
            NodeKind::NonNullExpr => {
                let Some(expr_node) = nodes::UnaryExpr::cast(source_tree, node)
                    .and_then(|expr| expr.operand(source_tree))
                else {
                    return true;
                };
                let expr_type = NodeRef::new(file, expr_node)
                    .cast(ctx, pkl_syntax::ast::nodes::Expr::cast)
                    .map_or(Type::Unknown, |expr| pkl_types::expr_type(expr, ctx));
                if !expr_type.is_nullable(ctx) {
                    holder.add_error(source_tree.span(node), "non null is redundant");
                    // TODO: add quickfix
                }
                false
            }
            _ => true,
        }
    }
}
