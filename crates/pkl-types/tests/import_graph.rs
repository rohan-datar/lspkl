mod harness;
use pkl_syntax::ast::{NodeKind, SyntaxTree};
use pkl_types::resolver::{module_type, resolve_type_name, resolve_unqualified_access};
use pkl_types::{AnalysisContext, FileId, ImportResolution, NodeRef, Type};
use std::{collections::HashMap, sync::Arc};
fn root(file: u32, tree: &SyntaxTree) -> NodeRef {
    NodeRef::new(FileId(file), tree.root())
}
fn graph(f: impl FnOnce(&AnalysisContext)) {
    let base = harness::load_test_base();
    let a=Arc::new(pkl_syntax::parse("extends \"b.pkl\"\nimport \"lib.pkl\" as library\nimport \"lib.pkl\"\nimport* \"*.pkl\" as libs\noverrideValue: String = \"a\"\nresult = library.value\ninstance: library.SomeClass\n").unwrap());
    let b = Arc::new(
        pkl_syntax::parse("amends \"a.pkl\"\ninherited: Int = 1\noverrideValue: Int = 2\n")
            .unwrap(),
    );
    let lib = Arc::new(pkl_syntax::parse("class SomeClass { field: Int }\nvalue = 42\n").unwrap());
    let mut edges = HashMap::new();
    for (file, t, target) in [(1, &a, root(2, &b)), (2, &b, root(1, &a))] {
        let c = harness::find_first(t, |k| matches!(k, NodeKind::ModuleExtendsAmendsClause));
        edges.insert(
            NodeRef::new(FileId(file), c),
            ImportResolution::Single(target),
        );
    }
    for id in pkl_syntax::ast::nodes::Module::cast(&a, a.root())
        .unwrap()
        .imports(&a)
    {
        edges.insert(
            NodeRef::new(FileId(1), id),
            if matches!(a.kind(id), NodeKind::ImportGlob) {
                ImportResolution::Glob(vec![root(3, &lib)])
            } else {
                ImportResolution::Single(root(3, &lib))
            },
        );
    }
    let trees = HashMap::from([(FileId(1), a.clone()), (FileId(2), b), (FileId(3), lib)]);
    let mut trees = trees;
    trees.insert(pkl_types::FileId(1), a.clone());
    trees.insert(base.file_id(), base.tree().clone());
    let ctx = AnalysisContext::new(&base, &trees, &edges);
    f(&ctx);
}
#[test]
fn aliases_module_values_and_qualified_types() {
    graph(|ctx| {
        for name in ["library", "lib"] {
            let r = resolve_unqualified_access(
                pkl_types::NodeRef::new(
                    pkl_types::FileId(1),
                    ctx.tree(pkl_types::FileId(1)).unwrap().root(),
                ),
                name,
                ctx,
            )
            .unwrap();
            assert_eq!(r.node.file, FileId(3));
            assert!(matches!(r.ty, Type::Module { .. }));
            let v = pkl_types::find_member(&r.ty, "value", ctx).unwrap();
            assert_eq!(v.ty, *ctx.base.int_type());
        }
        let ty = resolve_type_name(
            ctx,
            &["library", "SomeClass"],
            root(1, ctx.tree(pkl_types::FileId(1)).unwrap()),
        )
        .unwrap();
        assert_eq!(ty.file, FileId(3));
        let node = harness::find_first(ctx.tree(pkl_types::FileId(1)).unwrap(), |k| {
            matches!(k, NodeKind::QualifiedAccessExpr)
        });
        assert_eq!(
            pkl_types::expr_type(
                harness::expr(pkl_types::NodeRef::new(pkl_types::FileId(1), node), ctx),
                ctx
            ),
            *ctx.base.int_type()
        );
    });
}
#[test]
fn inherited_members_overrides_and_cycles() {
    graph(|ctx| {
        let ty = module_type(root(1, ctx.tree(pkl_types::FileId(1)).unwrap()), ctx);
        let inherited = pkl_types::find_member(&ty, "inherited", ctx).unwrap();
        assert_eq!(inherited.node.file, FileId(2));
        assert_eq!(inherited.ty, *ctx.base.int_type());
        let overridden = pkl_types::find_member(&ty, "overrideValue", ctx).unwrap();
        assert_eq!(overridden.node.file, FileId(1));
        assert_eq!(overridden.ty, *ctx.base.string_type());
        assert!(pkl_types::find_member(&ty, "missing", ctx).is_none());
        let parent = pkl_types::resolver::super_type(&ty, ctx).unwrap();
        assert_eq!(
            pkl_types::find_member(&parent, "overrideValue", ctx)
                .unwrap()
                .node
                .file,
            FileId(2)
        );
    });
}
#[test]
fn glob_import_is_mapping_of_modules() {
    graph(|ctx| {
        let ty = resolve_unqualified_access(
            pkl_types::NodeRef::new(
                pkl_types::FileId(1),
                ctx.tree(pkl_types::FileId(1)).unwrap().root(),
            ),
            "libs",
            ctx,
        )
        .unwrap()
        .ty;
        assert!(ty.class_equals(ctx.base.mapping_type()));
        let value = pkl_types::resolver::container_value_type(&ty, ctx).unwrap();
        assert!(matches!(value,Type::Module{module_node,..} if module_node.file==FileId(3)));
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
                root(1, ctx.tree(pkl_types::FileId(1)).unwrap()),
            )
            .unwrap();
            assert!(pkl_types::find_member(&Type::class(a), "missing", ctx).is_none());
            assert_eq!(
                resolve_unqualified_access(
                    pkl_types::NodeRef::new(
                        pkl_types::FileId(1),
                        ctx.tree(pkl_types::FileId(1)).unwrap().root()
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
    let tree=Arc::new(pkl_syntax::parse("extends \"parent.pkl\"\nx = import(\"lib.pkl\").value\ny = import*(\"*.pkl\")[\"lib.pkl\"].value\nz = super.value\n").unwrap());
    let target = Arc::new(pkl_syntax::parse("value: Int = 42\n").unwrap());
    let mut edges = HashMap::new();
    fn collect(t: &SyntaxTree, n: pkl_syntax::ast::NodeId, v: &mut Vec<pkl_syntax::ast::NodeId>) {
        if matches!(
            t.kind(n),
            NodeKind::ImportExpr | NodeKind::ModuleExtendsAmendsClause
        ) {
            v.push(n);
        }
        for &c in t.children(n) {
            collect(t, c, v);
        }
    }
    let mut nodes = Vec::new();
    collect(&tree, tree.root(), &mut nodes);
    for n in nodes {
        edges.insert(
            root(1, &tree).with_value(n),
            if tree.is_glob_import(n) {
                ImportResolution::Glob(vec![root(2, &target)])
            } else {
                ImportResolution::Single(root(2, &target))
            },
        );
    }
    let trees = HashMap::from([(FileId(2), target)]);
    let mut trees = trees;
    trees.insert(pkl_types::FileId(1), tree.clone());
    trees.insert(base.file_id(), base.tree().clone());
    let ctx = AnalysisContext::new(&base, &trees, &edges);
    for name in ["x", "y", "z"] {
        assert_eq!(
            resolve_unqualified_access(
                pkl_types::NodeRef::new(pkl_types::FileId(1), tree.root()),
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
        "import \"lib.pkl\" as library\nfunction f(library: Int): Int = library\nx = let (library = 42) library\n",
        |ctx| {
            let access = harness::find_first(ctx.tree(pkl_types::FileId(1)).unwrap(), |k| {
                matches!(k, NodeKind::UnqualifiedAccessExpr)
            });
            let resolved = resolve_unqualified_access(
                pkl_types::NodeRef::new(pkl_types::FileId(1), access),
                "library",
                ctx,
            )
            .unwrap();
            assert_eq!(resolved.ty, *base.int_type());
            assert_eq!(
                ctx.tree(pkl_types::FileId(1))
                    .unwrap()
                    .kind(resolved.node.value),
                &NodeKind::TypedIdentifier
            );

            let expr = harness::find_first(ctx.tree(pkl_types::FileId(1)).unwrap(), |k| {
                matches!(k, NodeKind::LetExpr)
            });
            assert_eq!(
                pkl_types::expr_type(
                    harness::expr(pkl_types::NodeRef::new(pkl_types::FileId(1), expr), ctx),
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
    let reference = NodeRef::new(FileId(1), node);
    let failure = ImportResolution::Unresolved {
        package_to_download: Some("package://example.com/missing@1.0.0".into()),
    };
    assert!(failure.single().is_none());
    let imports = HashMap::from([(reference, failure)]);
    let trees = HashMap::new();
    let mut trees = trees;
    trees.insert(pkl_types::FileId(1), tree.clone());
    trees.insert(base.file_id(), base.tree().clone());
    let ctx = AnalysisContext::new(&base, &trees, &imports);
    assert_eq!(
        pkl_types::resolver::import_type(reference, &ctx),
        Type::Unknown
    );
}
