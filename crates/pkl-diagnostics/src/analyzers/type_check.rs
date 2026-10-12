//! Type checking — checks expression types against declared types,
//! property defaults, function return types, parameter types, etc. The
//! workhorse analyzer.

use pkl_syntax::ast::{NodeId, nodes};
use pkl_types::compute_expr::compute_expr_type;
use pkl_types::infer_expr_type::infer_expr_type_from_context;
use pkl_types::{AnalysisContext, Type, TypeBindings};

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;

pub struct TypeCheckAnalyzer;

impl Analyzer for TypeCheckAnalyzer {
    fn analyze_node(
        &self,
        node: NodeId,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        if !nodes::Expr::can_cast(ctx.tree.kind(node)) {
            return true;
        }

        let bindings = TypeBindings::new();
        let expected = infer_expr_type_from_context(node, &bindings, ctx);
        if matches!(expected, Type::Unknown) {
            return true;
        }

        let actual = compute_expr_type(node, &bindings, ctx);
        if actual.is_subtype_of(&expected, ctx) {
            return true;
        }

        let span = ctx.tree.span(node);
        let render_actual = actual.render(ctx);
        let render_expected = expected.render(ctx);

        if !actual.has_common_subtype_with(&expected, ctx) {
            holder.add_error(
                span,
                format!("Type mismatch. Expected: {render_expected}. Actual: {render_actual}"),
            );
        } else if actual.is_nullable(ctx) && actual.non_null(ctx).is_subtype_of(&expected, ctx) {
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
