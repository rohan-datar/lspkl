//! Type checking — checks expression types against declared types,
//! property defaults, function return types, parameter types, etc. The
//! workhorse analyzer.

use pkl_types::NodeRef;

use pkl_syntax::ast::nodes;

use pkl_types::{AnalysisContext, Type};

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
            .cast(ctx, pkl_syntax::ast::nodes::Expr::cast)
            .map_or(Type::Unknown, |expr| {
                pkl_types::expected_type(ctx.db, expr.map(|expr| expr.id()), ctx.root)
            });
        if matches!(expected, Type::Unknown) {
            return true;
        }

        let actual = NodeRef::new(file, node)
            .cast(ctx, pkl_syntax::ast::nodes::Expr::cast)
            .map_or(Type::Unknown, |expr| {
                pkl_types::expr_type(ctx.db, expr, ctx.root)
            });
        if actual.is_subtype_of(&expected, ctx.db) {
            return true;
        }

        let span = source_tree.span(node);
        let render_actual = actual.render(ctx.db);
        let render_expected = expected.render(ctx.db);

        if !actual.has_common_subtype_with(&expected, ctx.db) {
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
