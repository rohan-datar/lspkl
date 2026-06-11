//! Type checking — checks expression types against declared types,
//! property defaults, function return types, parameter types, etc. The
//! workhorse analyzer.
//!
//! Mirrors `TypeCheckAnalyzer.kt`. Needs the full type-system stack:
//!   - `to_type` for converting `PklType` annotations to resolved `Type`
//!   - `compute_expr_type` for inferring expression types
//!   - `is_subtype_of` (full version, with subclass walk) for the assignability check
//!
//! The analyzer of last resort — implement after every other analyzer is in
//! place, because the type-system gaps it surfaces are rabbit-holes.
//!
//! Fixtures: `tests/snapshots/diagnostics/inputs/typecheck/*.pkl`.

use pkl_analysis::ast::NodeId;
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
        if !ctx.tree.kind(node).is_expr_node() {
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
