//! `compute_expr_type` tests.
//!
//! Each test parses a tiny Pkl snippet, finds the expression we care about,
//! and asserts on its inferred type.

mod harness;

use pkl_analysis::ast::NodeKind;
use pkl_types::{TypeBindings, compute_expr};

#[test]
fn int_literal_has_int_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = 42", |ctx| {
        let node = harness::find_first(ctx.tree, |k| matches!(k, NodeKind::IntLiteralExpr));
        let bindings = TypeBindings::new();
        let ty = compute_expr::compute_expr_type(node, &bindings, ctx);
        assert_eq!(&ty, ctx.base.int_type());
    });
}

#[test]
fn float_literal_has_float_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = 3.14", |ctx| {
        let node = harness::find_first(ctx.tree, |k| matches!(k, NodeKind::FloatLiteralExpr));
        let bindings = TypeBindings::new();
        let ty = compute_expr::compute_expr_type(node, &bindings, ctx);
        assert_eq!(&ty, ctx.base.float_type());
    });
}

#[test]
fn bool_literals_have_boolean_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "t = true\nf = false", |ctx| {
        let bindings = TypeBindings::new();

        let true_node = harness::find_first(ctx.tree, |k| matches!(k, NodeKind::TrueLiteralExpr));
        assert_eq!(
            &compute_expr::compute_expr_type(true_node, &bindings, ctx),
            ctx.base.boolean_type()
        );

        let false_node = harness::find_first(ctx.tree, |k| matches!(k, NodeKind::FalseLiteralExpr));
        assert_eq!(
            &compute_expr::compute_expr_type(false_node, &bindings, ctx),
            ctx.base.boolean_type()
        );
    });
}

#[test]
fn null_literal_has_null_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = null", |ctx| {
        let node = harness::find_first(ctx.tree, |k| matches!(k, NodeKind::NullLiteralExpr));
        let bindings = TypeBindings::new();
        let ty = compute_expr::compute_expr_type(node, &bindings, ctx);
        assert_eq!(&ty, ctx.base.null_type());
    });
}

#[test]
fn string_literal_without_interpolation_has_literal_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, r#"x = "hello""#, |ctx| {
        let node =
            harness::find_first(ctx.tree, |k| matches!(k, NodeKind::SingleLineStringLiteral));
        let bindings = TypeBindings::new();
        let ty = compute_expr::compute_expr_type(node, &bindings, ctx);
        assert_eq!(ty, pkl_types::Type::string_literal("hello"));
    });
}

#[test]
fn throw_expr_has_nothing_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, r#"x = throw("boom")"#, |ctx| {
        let node = harness::find_first(ctx.tree, |k| matches!(k, NodeKind::ThrowExpr));
        let bindings = TypeBindings::new();
        let ty = compute_expr::compute_expr_type(node, &bindings, ctx);
        assert_eq!(ty, pkl_types::Type::Nothing);
    });
}

#[test]
fn if_expr_unions_its_branches() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, r#"x = if (true) 1 else "two""#, |ctx| {
        let node = harness::find_first(ctx.tree, |k| matches!(k, NodeKind::IfExpr));
        let bindings = TypeBindings::new();
        let ty = compute_expr::compute_expr_type(node, &bindings, ctx);

        // Should be Int | "two" (or Int | String — depends on whether you keep
        // the literal type for the else branch). Whichever you choose, both
        // sub-types must be subtypes of the inferred type.
        assert!(ctx.base.int_type().is_subtype_of(&ty, ctx));
    });
}
