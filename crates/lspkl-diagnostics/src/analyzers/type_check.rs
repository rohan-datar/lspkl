//! Type checking — checks expression types against declared types,
//! property defaults, function return types, parameter types, etc. The
//! workhorse analyzer.

use lspkl_types::NodeRef;

use lspkl_syntax::ast::nodes;

use lspkl_types::{AnalysisContext, Type};

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;

pub struct TypeCheckAnalyzer;

impl Analyzer for TypeCheckAnalyzer {
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
        if !nodes::Expr::can_cast(source_tree.kind(node)) {
            return true;
        }

        let expected = NodeRef::new(file, node)
            .cast(ctx, lspkl_syntax::ast::nodes::Expr::cast)
            .map_or(Type::Unknown, |expr| {
                lspkl_types::expected_type(ctx.db, expr.map(|expr| expr.id()), ctx.root)
            });
        if matches!(expected, Type::Unknown) {
            return true;
        }

        let actual = NodeRef::new(file, node)
            .cast(ctx, lspkl_syntax::ast::nodes::Expr::cast)
            .map_or(Type::Unknown, |expr| {
                lspkl_types::expr_type(ctx.db, expr, ctx.root)
            });
        if actual.is_subtype_of(&expected, ctx.db) {
            return true;
        }

        let span = source_tree.span(node);
        let render_actual = actual.render(ctx.db);
        let render_expected = expected.render(ctx.db);

        let class_value = is_exact_class_value(NodeRef::new(file, node), ctx);
        if class_value || !actual.has_common_subtype_with(&expected, ctx.db) {
            holder.add_error(
                span,
                format!("Type mismatch. Expected: {render_expected}. Actual: {render_actual}"),
            );
        } else if actual.is_nullable(ctx.db)
            && actual.non_null(ctx.db).is_subtype_of(&expected, ctx.db)
        {
            holder.add_warning(
                span,
                format!(
                    "Nullability mismatch. Expected: {render_expected}. Actual: {render_actual}"
                ),
            );
        } else {
            holder.add_warning(
                span,
                format!("Type mismatch. Expected: {render_expected}. Actual: {render_actual}"),
            );
        }

        true
    }
}

/// A direct class reference is an exact runtime class, unlike a Class<T> variable.
pub(super) fn is_exact_class_value(node: NodeRef, ctx: &AnalysisContext) -> bool {
    let Some(tree) = ctx.tree(node.file) else {
        return false;
    };
    matches!(
        tree.kind(node.value),
        lspkl_syntax::ast::NodeKind::UnqualifiedAccessExpr
            | lspkl_syntax::ast::NodeKind::QualifiedAccessExpr
    ) && lspkl_types::resolver::resolve_reference(node, tree.span(node.value).start(), ctx)
        .and_then(|definition| lspkl_types::decl_for_node(ctx.db, definition, ctx.root))
        .is_some_and(|decl| matches!(decl.item, lspkl_types::ItemId::Class(_)))
}
