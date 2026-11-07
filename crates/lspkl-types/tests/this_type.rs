mod harness;

use lspkl_syntax::ast::{NodeKind, nodes};
use lspkl_types::{NodeRef, Type};

#[test]
fn this_annotation_resolves_enclosing_class() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "class Node { parent: this? }", |ctx| {
        let file = harness::file(ctx, 1);
        let tree = ctx.tree(file).unwrap();
        let node = harness::find_first(tree, |k| matches!(k, NodeKind::ThisType));
        let ty = lspkl_types::to_type(
            ctx.db,
            NodeRef::new(file, node)
                .cast(ctx, nodes::TypeNode::cast)
                .unwrap(),
            ctx.root,
        );
        assert_eq!(ty.render(ctx.db), "Node");
    });
}

#[test]
fn this_annotation_at_module_level_resolves_module() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "self: this = this", |ctx| {
        let file = harness::file(ctx, 1);
        let tree = ctx.tree(file).unwrap();
        let node = harness::find_first(tree, |k| matches!(k, NodeKind::ThisType));
        let ty = lspkl_types::to_type(
            ctx.db,
            NodeRef::new(file, node)
                .cast(ctx, nodes::TypeNode::cast)
                .unwrap(),
            ctx.root,
        );
        assert!(matches!(ty, Type::Module { .. }));
    });
}

#[test]
fn inherited_this_members_take_receiver_type() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "open class Node { parent: this?; children: List<this> }\nclass SpecialNode extends Node { special: Boolean }\nn = new SpecialNode {}\nx = n.children",
        |ctx| {
            let file = harness::file(ctx, 1);
            let tree = ctx.tree(file).unwrap();
            let node = harness::find_first(tree, |k| matches!(k, NodeKind::QualifiedAccessExpr));
            let ty = lspkl_types::expr_type(
                ctx.db,
                harness::expr(NodeRef::new(file, node), ctx),
                ctx.root,
            );
            assert_eq!(ty.render(ctx.db), "List<SpecialNode>");
        },
    );
}

#[test]
fn inherited_this_method_result_takes_receiver_type() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "open class Node { function copy(): this = this }\nclass Special extends Node {}\nn = new Special {}\nx = n.copy()",
        |ctx| {
            let file = harness::file(ctx, 1);
            let tree = ctx.tree(file).unwrap();
            let node = harness::find_first(tree, |k| matches!(k, NodeKind::QualifiedAccessExpr));
            let ty = lspkl_types::expr_type(
                ctx.db,
                harness::expr(NodeRef::new(file, node), ctx),
                ctx.root,
            );
            assert_eq!(ty.render(ctx.db), "Special");
        },
    );
}

#[test]
fn sdk_get_class_self_type_takes_receiver_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x: Int = 1\ny = x.getClass()", |ctx| {
        let file = harness::file(ctx, 1);
        let tree = ctx.tree(file).unwrap();
        let node = harness::find_first(tree, |k| matches!(k, NodeKind::QualifiedAccessExpr));
        let ty = lspkl_types::expr_type(
            ctx.db,
            harness::expr(NodeRef::new(file, node), ctx),
            ctx.root,
        );
        assert_eq!(ty.render(ctx.db), "Class<Int>");
    });
}

#[test]
fn this_in_typealias_and_unmodeled_object_stays_unknown() {
    let base = harness::load_test_base();
    for source in [
        "typealias Self = this",
        "obj = new Dynamic { self: this = this }",
    ] {
        harness::with_snippet(&base, source, |ctx| {
            let file = harness::file(ctx, 1);
            let tree = ctx.tree(file).unwrap();
            let node = harness::find_first(tree, |k| matches!(k, NodeKind::ThisType));
            let ty = lspkl_types::to_type(
                ctx.db,
                NodeRef::new(file, node)
                    .cast(ctx, nodes::TypeNode::cast)
                    .unwrap(),
                ctx.root,
            );
            assert_eq!(ty, Type::Unknown);
        });
    }
}
