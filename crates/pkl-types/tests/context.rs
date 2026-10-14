//! Pinned file isolation and file-qualified semantic inputs.
mod harness;
use pkl_syntax::ast::{NodeKind, nodes};
use pkl_types::{AnalysisContext, NodeRef, Type};
use std::sync::Arc;

#[test]
fn pinned_files_preserve_identity_and_isolate_resolution() {
    let base = harness::load_test_base();
    let first = Arc::new(pkl_syntax::parse("class Same { value: Int }\nx = 1\ny = x").unwrap());
    let second =
        Arc::new(pkl_syntax::parse("class Same { value: String }\nx = \"two\"\ny = x").unwrap());
    base.add(1, first.source());
    base.add(2, second.source());
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, pkl_db::Db::file(&*db, &harness::uri(1)).unwrap());
    for (file, tree, expected) in [
        (harness::file(&ctx, 1), &first, base.int_type().clone()),
        (harness::file(&ctx, 2), &second, Type::string_literal("two")),
    ] {
        let access =
            harness::find_first(tree, |kind| matches!(kind, NodeKind::UnqualifiedAccessExpr));
        let reference = NodeRef::new(file, access);
        assert_eq!(
            pkl_types::expr_type(
                ctx.db,
                reference.cast(&ctx, nodes::Expr::cast).unwrap(),
                ctx.root
            ),
            expected
        );
        let resolved =
            pkl_types::resolve_reference(reference, tree.span(access).start(), &ctx).unwrap();
        assert_eq!(resolved.file, file);
        let class = harness::find_first(tree, |kind| matches!(kind, NodeKind::Class));
        let member = pkl_types::find_member(
            ctx.db,
            &Type::class(
                pkl_types::class_id(ctx.db, NodeRef::new(file, class), ctx.root)
                    .expect("class declaration"),
            ),
            "value",
        )
        .unwrap();
        assert_eq!(member.node.file, file);
        assert_eq!(
            member.ty,
            if file == harness::file(&ctx, 1) {
                base.int_type().clone()
            } else {
                base.string_type().clone()
            }
        );
    }
}

#[test]
fn missing_files_never_fall_back_to_another_pinned_tree() {
    let base = harness::load_test_base();
    let tree = Arc::new(pkl_syntax::parse("x = 1").unwrap());
    base.add(1, tree.source());
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, pkl_db::Db::file(&*db, &harness::uri(1)).unwrap());
    let node = harness::find_first(&tree, |kind| matches!(kind, NodeKind::IntLiteralExpr));
    let missing = NodeRef::new(harness::file(&ctx, 3), node);
    assert!(ctx.tree(missing.file).is_none());
    assert!(missing.cast(&ctx, nodes::Expr::cast).is_none());
    assert_eq!(
        pkl_types::this_type(ctx.db, missing, ctx.root),
        Type::Unknown
    );
    assert_eq!(
        pkl_types::resolver::definition_type(ctx.db, missing, ctx.root),
        Type::Unknown
    );
}

#[test]
fn imported_generic_members_and_expected_arguments_keep_bindings_internal() {
    let base = harness::load_test_base();
    let user = Arc::new(
        pkl_syntax::parse(
            "import \"2.pkl\" as lib\nbox: lib.Box<Int>\nresult = box.value\ncall = box.accept(42)",
        )
        .unwrap(),
    );
    let imported = Arc::new(
        pkl_syntax::parse("class Box<T> { value: T\nfunction accept(arg: T): T = arg }").unwrap(),
    );
    base.add(1, user.source());
    base.add(2, imported.source());
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, pkl_db::Db::file(&*db, &harness::uri(1)).unwrap());
    let box_type = harness::find_first(&user, |kind| matches!(kind, NodeKind::DeclaredType));
    let ty = pkl_types::to_type(
        ctx.db,
        NodeRef::new(harness::file(&ctx, 1), box_type)
            .cast(&ctx, nodes::TypeNode::cast)
            .unwrap(),
        ctx.root,
    );
    let member = pkl_types::find_member(ctx.db, &ty, "value").unwrap();
    assert_eq!(member.node.file, harness::file(&ctx, 2));
    assert_eq!(member.ty, *base.int_type());
    let members = pkl_types::members_of_type(ctx.db, &ty);
    assert_eq!(
        members
            .iter()
            .find(|member| member.name == "value")
            .unwrap()
            .ty,
        *base.int_type()
    );
    let literal = harness::find_first(&user, |kind| matches!(kind, NodeKind::IntLiteralExpr));
    let expression = NodeRef::new(harness::file(&ctx, 1), literal)
        .cast(&ctx, nodes::Expr::cast)
        .unwrap();
    assert_eq!(
        pkl_types::expected_type(ctx.db, expression.map(|expr| expr.id()), ctx.root),
        *base.int_type()
    );
}

#[test]
fn enumeration_preserves_imported_inheritance_shadowing_and_union_intersection() {
    let base = harness::load_test_base();
    let child =
        Arc::new(pkl_syntax::parse("extends \"2.pkl\"\nvalue: String\nlocal: Boolean").unwrap());
    let parent = Arc::new(pkl_syntax::parse("value: Int\ninherited: Int").unwrap());
    base.add(1, child.source());
    base.add(2, parent.source());
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, pkl_db::Db::file(&*db, &harness::uri(1)).unwrap());
    let child_type =
        pkl_types::resolver::module_type(NodeRef::new(harness::file(&ctx, 1), child.root()), &ctx);
    let parent_type =
        pkl_types::resolver::module_type(NodeRef::new(harness::file(&ctx, 2), parent.root()), &ctx);
    let members = pkl_types::members_of_type(ctx.db, &child_type);
    assert_eq!(
        members
            .iter()
            .filter(|member| member.name == "value")
            .count(),
        1
    );
    let value = members
        .iter()
        .find(|member| member.name == "value")
        .unwrap();
    assert_eq!(value.node.file, harness::file(&ctx, 1));
    assert_eq!(value.ty, *base.string_type());
    assert_eq!(
        members
            .iter()
            .find(|member| member.name == "inherited")
            .unwrap()
            .node
            .file,
        harness::file(&ctx, 2)
    );
    let union = Type::union([child_type, parent_type], ctx.db);
    let members = pkl_types::members_of_type(ctx.db, &union);
    assert!(members.iter().any(|member| member.name == "inherited"));
    assert!(!members.iter().any(|member| member.name == "local"));
}
