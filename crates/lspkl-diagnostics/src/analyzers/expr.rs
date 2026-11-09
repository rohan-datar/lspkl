//! Expression-level checks: always-true / always-false conditions, redundant
//! type casts, redundant null-safe access, etc.

use lspkl_types::NodeRef;

use lspkl_syntax::ast::{NodeKind, nodes};

use lspkl_types::{AnalysisContext, Type};

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;
use crate::{QuickFix, TextEdit};

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
                    .cast(ctx, lspkl_syntax::ast::nodes::Expr::cast)
                    .map_or(Type::Unknown, |expr| {
                        lspkl_types::expr_type(ctx.db, expr, ctx.root)
                    });
                let tested_type = NodeRef::new(file, type_node)
                    .cast(ctx, lspkl_syntax::ast::nodes::TypeNode::cast)
                    .map_or(Type::Unknown, |ty| {
                        lspkl_types::to_type(ctx.db, ty, ctx.root)
                    });
                if tested_type.is_non_null(ctx.base) {
                    if !tested_type.constraints().is_empty() {
                        return false;
                    }
                    if !expr_type.is_nullable(ctx.db) || expr_type.is_non_null(ctx.base) {
                        holder.add_warning(source_tree.span(node), "expression is always true");
                    }
                } else {
                    if tested_type.has_constraints(ctx.db) {
                        return false;
                    }
                    if !matches!(expr_type, Type::Unknown)
                        && expr_type.is_subtype_of(&tested_type, ctx.db)
                    {
                        holder.add_warning(source_tree.span(node), "expression is always true");
                    } else if super::type_check::is_exact_class_value(
                        NodeRef::new(file, expr_node),
                        ctx,
                    ) && !matches!(tested_type, Type::Unknown)
                        || !tested_type.is_subtype_of(&expr_type, ctx.db)
                    {
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
                    .cast(ctx, lspkl_syntax::ast::nodes::Expr::cast)
                    .map_or(Type::Unknown, |expr| {
                        lspkl_types::expr_type(ctx.db, expr, ctx.root)
                    });
                let tested_type = NodeRef::new(file, type_node)
                    .cast(ctx, lspkl_syntax::ast::nodes::TypeNode::cast)
                    .map_or(Type::Unknown, |ty| {
                        lspkl_types::to_type(ctx.db, ty, ctx.root)
                    });

                let known_non_null = tested_type.is_non_null(ctx.base)
                    && tested_type.constraints().is_empty()
                    && (!expr_type.is_nullable(ctx.db) || expr_type.is_non_null(ctx.base));
                if (!tested_type.has_constraints(ctx.db) || known_non_null)
                    && !matches!(expr_type, Type::Unknown)
                    && expr_type.is_subtype_of(&tested_type, ctx.db)
                {
                    holder.add_error_with_fix(
                        source_tree.span(node),
                        "type cast is redundant",
                        QuickFix::single(
                            "Remove redundant type cast",
                            surviving_operand(source_tree, node, expr_node),
                        ),
                    );
                } else if (super::type_check::is_exact_class_value(
                    NodeRef::new(file, expr_node),
                    ctx,
                ) && !matches!(tested_type, Type::Unknown)
                    && !expr_type.is_subtype_of(&tested_type, ctx.db))
                    || (!tested_type.is_subtype_of(&expr_type, ctx.db)
                        && !tested_type.has_common_subtype_with(&expr_type, ctx.db))
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
                    .cast(ctx, lspkl_syntax::ast::nodes::Expr::cast)
                    .map_or(Type::Unknown, |expr| {
                        lspkl_types::expr_type(ctx.db, expr, ctx.root)
                    });
                if !expr_type.is_nullable(ctx.db) {
                    holder.add_error_with_fix(
                        source_tree.span(node),
                        "null coalescing is redundant",
                        QuickFix::single(
                            "Remove redundant null coalescing",
                            surviving_operand(source_tree, node, expr_node),
                        ),
                    );
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
                    .cast(ctx, lspkl_syntax::ast::nodes::Expr::cast)
                    .map_or(Type::Unknown, |expr| {
                        lspkl_types::expr_type(ctx.db, expr, ctx.root)
                    });
                if !expr_type.is_nullable(ctx.db) {
                    holder.add_error_with_fix(
                        source_tree.span(node),
                        "non null is redundant",
                        QuickFix::single(
                            "Remove redundant non-null assertion",
                            surviving_operand(source_tree, node, expr_node),
                        ),
                    );
                }
                false
            }
            _ => true,
        }
    }
}

/// Preserve the operand's exact source, including any existing grouping.
fn surviving_operand(
    tree: &lspkl_syntax::ast::SyntaxTree,
    removed: lspkl_syntax::ast::NodeId,
    operand: lspkl_syntax::ast::NodeId,
) -> TextEdit {
    let grouped = tree.parent(removed).is_some_and(|parent| {
        let child_precedence = precedence(tree.kind(operand));
        if let Some(binary) = nodes::BinaryExpr::cast(tree, parent) {
            let parent_precedence = precedence(tree.kind(parent));
            let right_associative = matches!(
                tree.kind(parent),
                NodeKind::NullCoalesceExpr | NodeKind::ExponentiationExpr
            );
            child_precedence < parent_precedence
                || (child_precedence == parent_precedence
                    && if right_associative {
                        binary.left(tree) == Some(removed)
                    } else {
                        binary.right(tree) == Some(removed)
                    })
        } else if nodes::UnaryExpr::cast(tree, parent).is_some()
            || nodes::QualifiedAccessExpr::cast(tree, parent)
                .and_then(|access| access.receiver(tree))
                == Some(removed)
            || nodes::SubscriptExpr::cast(tree, parent).and_then(|access| access.receiver(tree))
                == Some(removed)
        {
            child_precedence < precedence(tree.kind(parent))
        } else {
            false
        }
    });
    let text = tree.text(operand);
    TextEdit::replace(
        tree.span(removed),
        if grouped {
            format!("({text})")
        } else {
            text.to_owned()
        },
    )
}

// Binding powers from the Pkl expression grammar. Delimited expressions and
// atoms bind more tightly than operators; existing parentheses remain intact.
fn precedence(kind: &NodeKind) -> u8 {
    match kind {
        NodeKind::FunctionLiteralExpr | NodeKind::IfExpr | NodeKind::LetExpr => 0,
        NodeKind::NullCoalesceExpr => 8,
        NodeKind::PipeExpr => 9,
        NodeKind::LogicalOrExpr => 10,
        NodeKind::LogicalAndExpr => 11,
        NodeKind::EqualityExpr => 12,
        NodeKind::TypeTestExpr | NodeKind::TypeCastExpr => 14,
        NodeKind::ComparisonExpr => 15,
        NodeKind::AdditiveExpr => 16,
        NodeKind::MultiplicativeExpr => 17,
        NodeKind::ExponentiationExpr => 18,
        NodeKind::LogicalNotExpr => 19,
        NodeKind::UnaryMinusExpr => 20,
        NodeKind::NonNullExpr => 21,
        NodeKind::QualifiedAccessExpr | NodeKind::SubscriptExpr => 22,
        _ => 23,
    }
}
