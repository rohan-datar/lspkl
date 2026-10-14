//! `compute_this_type` tests.

mod harness;

use pkl_syntax::ast::NodeKind;
use pkl_types::Type;

#[test]
fn this_type_at_module_top_is_module() {
    let base = harness::load_test_base();
    // Module-level property; the `1` literal lives directly under Module.
    harness::with_snippet(&base, "x = 1", |ctx| {
        let lit = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::IntLiteralExpr)
        });
        let ty = pkl_types::this_type(
            ctx.db,
            pkl_types::NodeRef::new(harness::file(ctx, 1), lit),
            ctx.root,
        );

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
        let lit = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::IntLiteralExpr)
        });
        let ty = pkl_types::this_type(
            ctx.db,
            pkl_types::NodeRef::new(harness::file(ctx, 1), lit),
            ctx.root,
        );

        // Should be Type::Class pointing at Foo.
        let Type::Class { class_id, .. } = &ty else {
            panic!("expected Type::Class, got {ty:?}");
        };

        // Verify it's actually pointing at `Foo` by checking the name.
        let items = pkl_types::module_items(ctx.db, class_id.module(ctx.db).file(ctx.db));
        assert_eq!(
            items.classes[class_id.index(ctx.db)]
                .declaration
                .name
                .text(ctx.db),
            "Foo"
        );
    });
}

#[test]
fn this_type_inside_constrained_type_is_inner_type() {
    let base = harness::load_test_base();
    // Inside `Int(this > 0)`, `this` should resolve to the type being
    // constrained — `Int` — not to the enclosing module/typealias.
    harness::with_snippet(&base, "typealias Bar = Int(this > 0)", |ctx| {
        let inner = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::ThisExpr)
        });
        let ty = pkl_types::this_type(
            ctx.db,
            pkl_types::NodeRef::new(harness::file(ctx, 1), inner),
            ctx.root,
        );

        // Should be Type::Class pointing at Int (which lives in pkl:base).
        assert!(
            ty.class_equals(ctx.base.int_type()),
            "expected Type::Class for Int, got {ty:?}"
        );
    });
}
