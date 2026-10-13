mod harness;
use pkl_syntax::ast::NodeKind;
use pkl_types::resolver::{module_type, resolve_type_name, resolve_unqualified_access};
use pkl_types::{AnalysisContext, NodeRef, Type};
use std::sync::Arc;
fn graph(f: impl FnOnce(&AnalysisContext)) {
    let base = harness::load_test_base();
    let a=Arc::new(pkl_syntax::parse("extends \"2.pkl\"\nimport \"3.pkl\" as library\nimport \"3.pkl\"\nimport* \"3*.pkl\" as libs\noverrideValue: String = \"a\"\nresult = library.value\ninstance: library.SomeClass\n").unwrap());
    let b = Arc::new(
        pkl_syntax::parse("amends \"1.pkl\"\ninherited: Int = 1\noverrideValue: Int = 2\n")
            .unwrap(),
    );
    let lib = Arc::new(pkl_syntax::parse("class SomeClass { field: Int }\nvalue = 42\n").unwrap());
    base.add(1, a.source());
    base.add(2, b.source());
    base.add(3, lib.source());
    let db = base.db.borrow();
    let ctx = AnalysisContext::new(
        &base,
        &*db,
        pkl_db::Db::file(&*db, &harness::uri(1)).unwrap(),
    );
    f(&ctx);
}
#[test]
fn aliases_module_values_and_qualified_types() {
    graph(|ctx| {
        for name in ["library", "3"] {
            let r = resolve_unqualified_access(
                pkl_types::NodeRef::new(
                    harness::file(ctx, 1),
                    ctx.tree(harness::file(ctx, 1)).unwrap().root(),
                ),
                name,
                ctx,
            )
            .unwrap();
            assert_eq!(r.node.file, harness::file(ctx, 3));
            assert!(matches!(r.ty, Type::Module { .. }));
            let v = pkl_types::find_member(&r.ty, "value", ctx).unwrap();
            assert_eq!(v.ty, *ctx.base.int_type());
        }
        let ty = resolve_type_name(
            ctx,
            &["library", "SomeClass"],
            NodeRef::new(harness::file(ctx, 1), pkl_syntax::ast::NodeId::ROOT),
        )
        .unwrap();
        assert_eq!(ty.file, harness::file(ctx, 3));
        let node = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::QualifiedAccessExpr)
        });
        assert_eq!(
            pkl_types::expr_type(
                harness::expr(pkl_types::NodeRef::new(harness::file(ctx, 1), node), ctx),
                ctx
            ),
            *ctx.base.int_type()
        );
    });
}
#[test]
fn inherited_members_overrides_and_cycles() {
    graph(|ctx| {
        let ty = module_type(
            NodeRef::new(harness::file(ctx, 1), pkl_syntax::ast::NodeId::ROOT),
            ctx,
        );
        let inherited = pkl_types::find_member(&ty, "inherited", ctx).unwrap();
        assert_eq!(inherited.node.file, harness::file(ctx, 2));
        assert_eq!(inherited.ty, *ctx.base.int_type());
        let overridden = pkl_types::find_member(&ty, "overrideValue", ctx).unwrap();
        assert_eq!(overridden.node.file, harness::file(ctx, 1));
        assert_eq!(overridden.ty, *ctx.base.string_type());
        assert!(pkl_types::find_member(&ty, "missing", ctx).is_none());
        let parent = pkl_types::resolver::super_type(&ty, ctx).unwrap();
        assert_eq!(
            pkl_types::find_member(&parent, "overrideValue", ctx)
                .unwrap()
                .node
                .file,
            harness::file(ctx, 2)
        );
    });
}
#[test]
fn glob_import_is_mapping_of_modules() {
    graph(|ctx| {
        let ty = resolve_unqualified_access(
            pkl_types::NodeRef::new(
                harness::file(ctx, 1),
                ctx.tree(harness::file(ctx, 1)).unwrap().root(),
            ),
            "libs",
            ctx,
        )
        .unwrap()
        .ty;
        assert!(ty.class_equals(ctx.base.mapping_type()));
        let value = pkl_types::resolver::container_value_type(&ty, ctx).unwrap();
        assert!(
            matches!(value,Type::Module{module_node,..} if module_node.file==harness::file(ctx, 3))
        );
    });
}
#[test]
fn cyclic_classes_and_initializers_terminate() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "class A extends B {}\nclass B extends A {}\nx = y\ny = x\n",
        |ctx| {
            let a = resolve_type_name(
                ctx,
                &["A"],
                NodeRef::new(harness::file(ctx, 1), pkl_syntax::ast::NodeId::ROOT),
            )
            .unwrap();
            assert!(pkl_types::find_member(&Type::class(a), "missing", ctx).is_none());
            assert_eq!(
                resolve_unqualified_access(
                    pkl_types::NodeRef::new(
                        harness::file(ctx, 1),
                        ctx.tree(harness::file(ctx, 1)).unwrap().root()
                    ),
                    "x",
                    ctx
                )
                .unwrap()
                .ty,
                Type::Unknown
            );
        },
    );
}

#[test]
fn import_expressions_and_super_use_pinned_edges() {
    let base = harness::load_test_base();
    let tree=Arc::new(pkl_syntax::parse("extends \"2.pkl\"\nx = import(\"2.pkl\").value\ny = import*(\"2*.pkl\")[\"2.pkl\"].value\nz = super.value\n").unwrap());
    let target = Arc::new(pkl_syntax::parse("value: Int = 42\n").unwrap());
    base.add(1, tree.source());
    base.add(2, target.source());
    let db = base.db.borrow();
    let ctx = AnalysisContext::new(
        &base,
        &*db,
        pkl_db::Db::file(&*db, &harness::uri(1)).unwrap(),
    );
    for name in ["x", "y", "z"] {
        assert_eq!(
            resolve_unqualified_access(
                pkl_types::NodeRef::new(harness::file(&ctx, 1), tree.root()),
                name,
                &ctx
            )
            .unwrap()
            .ty,
            *base.int_type()
        );
    }
}
#[test]
fn imported_alias_is_shadowed_by_parameters_and_let_bindings() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "import \"2.pkl\" as library\nfunction f(library: Int): Int = library\nx = let (library = 42) library\n",
        |ctx| {
            let access = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
                matches!(k, NodeKind::UnqualifiedAccessExpr)
            });
            let resolved = resolve_unqualified_access(
                pkl_types::NodeRef::new(harness::file(ctx, 1), access),
                "library",
                ctx,
            )
            .unwrap();
            assert_eq!(resolved.ty, *base.int_type());
            assert_eq!(
                ctx.tree(harness::file(ctx, 1))
                    .unwrap()
                    .kind(resolved.node.value),
                &NodeKind::TypedIdentifier
            );

            let expr = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
                matches!(k, NodeKind::LetExpr)
            });
            assert_eq!(
                pkl_types::expr_type(
                    harness::expr(pkl_types::NodeRef::new(harness::file(ctx, 1), expr), ctx),
                    ctx
                ),
                *base.int_type()
            );
        },
    );
}

#[test]
fn unresolved_import_remains_unknown_not_an_empty_glob() {
    let tree =
        std::sync::Arc::new(pkl_syntax::parse(r#"import "missing.pkl" as missing"#).unwrap());
    let base = harness::load_test_base();
    let node = harness::find_first(&tree, |kind| matches!(kind, NodeKind::Import));
    base.add(1, tree.source());
    let db = base.db.borrow();
    let ctx = AnalysisContext::new(
        &base,
        &*db,
        pkl_db::Db::file(&*db, &harness::uri(1)).unwrap(),
    );
    let reference = NodeRef::new(harness::file(&ctx, 1), node);
    assert!(ctx.import(reference).unwrap().single().is_none());
    assert_eq!(
        pkl_types::resolver::import_type(reference, &ctx),
        Type::Unknown
    );
}
