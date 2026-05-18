//! `compute_this_type` tests.

mod harness;

use pkl_analysis::ast::NodeKind;
use pkl_types::{Type, TypeBindings, compute_expr};

#[test]
fn this_type_at_module_top_is_module() {
    let base = harness::load_test_base();
    // Module-level property; the `1` literal lives directly under Module.
    harness::with_snippet(&base, "x = 1", |ctx| {
        let lit = harness::find_first(ctx.tree, |k| matches!(k, NodeKind::IntLiteralExpr));
        let bindings = TypeBindings::new();
        let ty = compute_expr::compute_this_type(lit, &bindings, ctx);

        match ty {
            Type::Module { .. } => {} // good — `this` is the module
            other => panic!("expected Type::Module, got {other:?}"),
        }
    });
}

#[test]
fn this_type_inside_class_is_that_class() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "class Foo { x = 1 }", |ctx| {
        let lit = harness::find_first(ctx.tree, |k| matches!(k, NodeKind::IntLiteralExpr));
        let bindings = TypeBindings::new();
        let ty = compute_expr::compute_this_type(lit, &bindings, ctx);

        // Should be Type::Class pointing at Foo.
        let Type::Class { class_node, .. } = &ty else {
            panic!("expected Type::Class, got {ty:?}");
        };

        // Verify it's actually pointing at `Foo` by checking the name.
        let tree = ctx
            .tree_for(class_node.file_id)
            .expect("class lives in ctx");
        assert_eq!(tree.name(class_node.node_id), Some("Foo"));
    });
}

#[test]
fn this_type_inside_typealias_body_is_unknown() {
    let base = harness::load_test_base();
    // A typealias body is a type-syntax position; `this` has no useful type.
    // A-level returns Unknown.
    harness::with_snippet(&base, "typealias Bar = Int(this > 0)", |ctx| {
        // Find the `this` reference inside the constraint. ThisExpr is the
        // cleanest target; fall back to a literal if your grammar doesn't
        // surface it that way.
        let inner = harness::find_first(ctx.tree, |k| matches!(k, NodeKind::ThisExpr));
        let bindings = TypeBindings::new();
        let ty = compute_expr::compute_this_type(inner, &bindings, ctx);

        // A-level: returns Unknown because TypeAlias is one of the bailout cases.
        assert_eq!(ty, Type::Unknown);
    });
}

// ---------------------------------------------------------------------------
//  deferred until compute_this_type handles AmendExpr / NewExpr
//  ObjectBody.
// ---------------------------------------------------------------------------
//
// #[test]
// fn this_type_inside_new_object_body_matches_class() {
//     let base = harness::load_test_base();
//     harness::with_snippet(&base, "class Foo { x: Int } y = new Foo { x = 1 }", |ctx| {
//         // Find the `1` literal inside the `new Foo { ... }` body and assert
//         // compute_this_type returns Type::Class for Foo.
//     });
// }
//
// #[test]
// fn this_type_inside_amend_uses_receiver_type() {
//     let base = harness::load_test_base();
//     // amend syntax: `foo { ... }` where foo's declared type is the receiver
// }
