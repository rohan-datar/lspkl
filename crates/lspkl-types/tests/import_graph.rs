mod harness;
use lspkl_syntax::ast::NodeKind;
use lspkl_types::resolver::{module_type, resolve_type_name, resolve_unqualified_access};
use lspkl_types::{AnalysisContext, NodeRef, Type};
use std::sync::Arc;
fn graph(f: impl FnOnce(&AnalysisContext)) {
    let base = harness::load_test_base();
    let a=Arc::new(lspkl_syntax::parse("extends \"2.pkl\"\nimport \"3.pkl\" as library\nimport \"3.pkl\"\nimport* \"3*.pkl\" as libs\noverrideValue: String = \"a\"\nresult = library.value\ninstance: library.SomeClass\n").unwrap());
    let b = Arc::new(
        lspkl_syntax::parse("amends \"1.pkl\"\ninherited: Int = 1\noverrideValue: Int = 2\n")
            .unwrap(),
    );
    let lib =
        Arc::new(lspkl_syntax::parse("class SomeClass { field: Int }\nvalue = 42\n").unwrap());
    base.add(1, a.source());
    base.add(2, b.source());
    base.add(3, lib.source());
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, lspkl_db::Db::file(&*db, &harness::uri(1)).unwrap());
    f(&ctx);
}
#[test]
fn aliases_module_values_and_qualified_types() {
    graph(|ctx| {
        for name in ["library", "3"] {
            let r = resolve_unqualified_access(
                lspkl_types::NodeRef::new(
                    harness::file(ctx, 1),
                    ctx.tree(harness::file(ctx, 1)).unwrap().root(),
                ),
                name,
                ctx,
            )
            .unwrap();
            assert_eq!(r.node.file, harness::file(ctx, 3));
            assert!(matches!(r.ty, Type::Module { .. }));
            let v = lspkl_types::find_member(ctx.db, &r.ty, "value").unwrap();
            assert_eq!(v.ty, *ctx.base.int_type());
        }
        let ty = resolve_type_name(
            ctx,
            &["library", "SomeClass"],
            NodeRef::new(harness::file(ctx, 1), lspkl_syntax::ast::NodeId::ROOT),
        )
        .unwrap();
        assert_eq!(ty.file, harness::file(ctx, 3));
        let node = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::QualifiedAccessExpr)
        });
        assert_eq!(
            lspkl_types::expr_type(
                ctx.db,
                harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), node), ctx),
                ctx.root
            ),
            *ctx.base.int_type()
        );
    });
}
#[test]
fn inherited_members_overrides_and_cycles() {
    graph(|ctx| {
        let ty = module_type(
            NodeRef::new(harness::file(ctx, 1), lspkl_syntax::ast::NodeId::ROOT),
            ctx,
        );
        let inherited = lspkl_types::find_member(ctx.db, &ty, "inherited").unwrap();
        assert_eq!(inherited.node.file, harness::file(ctx, 2));
        assert_eq!(inherited.ty, *ctx.base.int_type());
        let overridden = lspkl_types::find_member(ctx.db, &ty, "overrideValue").unwrap();
        assert_eq!(overridden.node.file, harness::file(ctx, 1));
        assert_eq!(overridden.ty, *ctx.base.string_type());
        assert!(lspkl_types::find_member(ctx.db, &ty, "missing").is_none());
        let parent = lspkl_types::resolver::super_type(&ty, ctx.db).unwrap();
        assert_eq!(
            lspkl_types::find_member(ctx.db, &parent, "overrideValue")
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
            lspkl_types::NodeRef::new(
                harness::file(ctx, 1),
                ctx.tree(harness::file(ctx, 1)).unwrap().root(),
            ),
            "libs",
            ctx,
        )
        .unwrap()
        .ty;
        assert!(ty.class_equals(ctx.base.mapping_type()));
        let value = lspkl_types::resolver::container_value_type(&ty, ctx).unwrap();
        assert!(
            matches!(value,Type::Module{module_id,..} if module_id.file(ctx.db)==harness::file(ctx, 3))
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
                NodeRef::new(harness::file(ctx, 1), lspkl_syntax::ast::NodeId::ROOT),
            )
            .unwrap();
            assert!(
                lspkl_types::find_member(
                    ctx.db,
                    &Type::class(lspkl_types::class_id(ctx.db, a, ctx.root).unwrap()),
                    "missing"
                )
                .is_none()
            );
            assert_eq!(
                resolve_unqualified_access(
                    lspkl_types::NodeRef::new(
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
    let tree=Arc::new(lspkl_syntax::parse("extends \"2.pkl\"\nx = import(\"2.pkl\").value\ny = import*(\"2*.pkl\")[\"2.pkl\"].value\nz = super.value\n").unwrap());
    let target = Arc::new(lspkl_syntax::parse("value: Int = 42\n").unwrap());
    base.add(1, tree.source());
    base.add(2, target.source());
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, lspkl_db::Db::file(&*db, &harness::uri(1)).unwrap());
    for name in ["x", "y", "z"] {
        assert_eq!(
            resolve_unqualified_access(
                lspkl_types::NodeRef::new(harness::file(&ctx, 1), tree.root()),
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
                lspkl_types::NodeRef::new(harness::file(ctx, 1), access),
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
                lspkl_types::expr_type(
                    ctx.db,
                    harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), expr), ctx),
                    ctx.root
                ),
                *base.int_type()
            );
        },
    );
}

#[test]
fn unresolved_import_remains_unknown_not_an_empty_glob() {
    let tree =
        std::sync::Arc::new(lspkl_syntax::parse(r#"import "missing.pkl" as missing"#).unwrap());
    let base = harness::load_test_base();
    let node = harness::find_first(&tree, |kind| matches!(kind, NodeKind::Import));
    base.add(1, tree.source());
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, lspkl_db::Db::file(&*db, &harness::uri(1)).unwrap());
    let reference = NodeRef::new(harness::file(&ctx, 1), node);
    assert!(ctx.import(reference).unwrap().single().is_none());
    assert_eq!(
        lspkl_types::resolver::import_type(reference, &ctx),
        Type::Unknown
    );
}

#[test]
fn own_non_glob_imports_name_module_types() {
    let base = harness::load_test_base();
    base.add(2, "value: Int = 1\n");
    base.add(3, "import \"2.pkl\" as Inherited\nopen class Unused\n");
    base.add(
        1,
        "extends \"3.pkl\"\nimport \"2.pkl\" as Library\nimport* \"*.pkl\" as all\n\
         typed: Library?\nglobbed: all?\ninherited: Inherited?\n",
    );
    let db = base.db.borrow();
    let file = lspkl_db::Db::file(&*db, &harness::uri(1)).unwrap();
    let ctx = AnalysisContext::for_root(&*db, file);
    let at = NodeRef::new(file, lspkl_syntax::ast::NodeId::ROOT);
    let library = harness::file(&ctx, 2);

    let target = resolve_type_name(&ctx, &["Library"], at).unwrap();
    assert_eq!(target.file, library, "type names navigate into the module");
    let property = |name: &str| {
        let items = lspkl_types::module_items(ctx.db, file);
        let (index, _) = items
            .properties
            .iter()
            .find(|(_, property)| property.declaration.name.text(ctx.db) == name)
            .unwrap();
        lspkl_types::decl_type(
            ctx.db,
            lspkl_types::DeclId {
                file,
                root: file,
                item: lspkl_types::ItemId::Property(index),
            },
        )
    };
    assert_eq!(
        property("typed"),
        Type::Module {
            module_id: lspkl_types::module_id(ctx.db, library, file),
        }
        .nullable(ctx.db)
    );
    // Glob imports are mappings, not types; imports are not inherited.
    assert!(resolve_type_name(&ctx, &["all"], at).is_none());
    assert!(resolve_type_name(&ctx, &["Inherited"], at).is_none());
    assert_eq!(property("globbed"), Type::Unknown.nullable(ctx.db));
}

#[test]
fn imported_aliases_and_argument_context_preserve_lexical_scope() {
    let base = harness::load_test_base();
    base.add(
        2,
        "class C { field: Int }\ntypealias Alias = C\nvalue = 42\n\
         function f(arg: String): String = arg\n",
    );
    base.add(3, "output: String = \"parent\"\n");
    harness::with_snippet(
        &base,
        "extends \"3.pkl\"\nimport \"2.pkl\" as library\nimport \"2.pkl\" as output\n\
         x: library.Alias\nq = x.field\nz = output.value\n\
         y = let (library = library.value) library\nw = library.f(foobar)\n",
        |ctx| {
            let file = harness::file(ctx, 1);
            let at = NodeRef::new(file, ctx.tree(file).unwrap().root());
            let output = resolve_unqualified_access(at, "output", ctx).unwrap();
            assert_eq!(output.node.file, harness::file(ctx, 2));
            assert!(matches!(output.ty, Type::Module { .. }));
            for name in ["q", "z", "y"] {
                assert_eq!(
                    resolve_unqualified_access(at, name, ctx).unwrap().ty,
                    *ctx.base.int_type(),
                    "{name} must retain the imported Int type"
                );
            }
            let library = resolve_unqualified_access(at, "library", ctx).unwrap();
            let alias = lspkl_types::find_member(ctx.db, &library.ty, "Alias").unwrap();
            assert!(matches!(alias.ty, Type::Alias { .. }));
            assert_eq!(alias.node.file, harness::file(ctx, 2));
            fn find_argument(
                tree: &lspkl_syntax::ast::SyntaxTree,
                node: lspkl_syntax::ast::NodeId,
            ) -> Option<lspkl_syntax::ast::NodeId> {
                if matches!(tree.kind(node), NodeKind::UnqualifiedAccessExpr)
                    && tree.text(node) == "foobar"
                {
                    return Some(node);
                }
                tree.children(node)
                    .iter()
                    .find_map(|&child| find_argument(tree, child))
            }
            let argument = find_argument(ctx.tree(file).unwrap(), at.value).unwrap();
            let expression = harness::expr(NodeRef::new(file, argument), ctx);
            assert_eq!(
                lspkl_types::expected_type(ctx.db, expression.map(|expr| expr.id()), ctx.root),
                *ctx.base.string_type()
            );
        },
    );
}
