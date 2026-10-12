//! `compute_expr_type` tests.
//!
//! Each test parses a tiny Pkl snippet, finds the expression we care about,
//! and asserts on its inferred type.

mod harness;

use pkl_syntax::ast::NodeKind;

#[test]
fn int_literal_has_int_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = 42", |ctx| {
        let node = harness::find_first(ctx.tree(pkl_types::FileId(1)).unwrap(), |k| {
            matches!(k, NodeKind::IntLiteralExpr)
        });
        let ty = pkl_types::expr_type(
            harness::expr(pkl_types::NodeRef::new(pkl_types::FileId(1), node), ctx),
            ctx,
        );
        assert_eq!(&ty, ctx.base.int_type());
    });
}

#[test]
fn float_literal_has_float_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = 3.14", |ctx| {
        let node = harness::find_first(ctx.tree(pkl_types::FileId(1)).unwrap(), |k| {
            matches!(k, NodeKind::FloatLiteralExpr)
        });
        let ty = pkl_types::expr_type(
            harness::expr(pkl_types::NodeRef::new(pkl_types::FileId(1), node), ctx),
            ctx,
        );
        assert_eq!(&ty, ctx.base.float_type());
    });
}

#[test]
fn bool_literals_have_boolean_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "t = true\nf = false", |ctx| {
        let true_node = harness::find_first(ctx.tree(pkl_types::FileId(1)).unwrap(), |k| {
            matches!(k, NodeKind::TrueLiteralExpr)
        });
        assert_eq!(
            &pkl_types::expr_type(
                harness::expr(
                    pkl_types::NodeRef::new(pkl_types::FileId(1), true_node),
                    ctx
                ),
                ctx
            ),
            ctx.base.boolean_type()
        );

        let false_node = harness::find_first(ctx.tree(pkl_types::FileId(1)).unwrap(), |k| {
            matches!(k, NodeKind::FalseLiteralExpr)
        });
        assert_eq!(
            &pkl_types::expr_type(
                harness::expr(
                    pkl_types::NodeRef::new(pkl_types::FileId(1), false_node),
                    ctx
                ),
                ctx
            ),
            ctx.base.boolean_type()
        );
    });
}

#[test]
fn null_literal_has_null_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = null", |ctx| {
        let node = harness::find_first(ctx.tree(pkl_types::FileId(1)).unwrap(), |k| {
            matches!(k, NodeKind::NullLiteralExpr)
        });
        let ty = pkl_types::expr_type(
            harness::expr(pkl_types::NodeRef::new(pkl_types::FileId(1), node), ctx),
            ctx,
        );
        assert_eq!(&ty, ctx.base.null_type());
    });
}

#[test]
fn string_literal_without_interpolation_has_literal_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, r#"x = "hello""#, |ctx| {
        let node = harness::find_first(ctx.tree(pkl_types::FileId(1)).unwrap(), |k| {
            matches!(k, NodeKind::SingleLineStringLiteral)
        });
        let ty = pkl_types::expr_type(
            harness::expr(pkl_types::NodeRef::new(pkl_types::FileId(1), node), ctx),
            ctx,
        );
        assert_eq!(ty, pkl_types::Type::string_literal("hello"));
    });
}

#[test]
fn throw_expr_has_nothing_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, r#"x = throw("boom")"#, |ctx| {
        let node = harness::find_first(ctx.tree(pkl_types::FileId(1)).unwrap(), |k| {
            matches!(k, NodeKind::ThrowExpr)
        });
        let ty = pkl_types::expr_type(
            harness::expr(pkl_types::NodeRef::new(pkl_types::FileId(1), node), ctx),
            ctx,
        );
        assert_eq!(ty, pkl_types::Type::Nothing);
    });
}

#[test]
fn unqualified_access_resolves_to_module_property_type() {
    let base = harness::load_test_base();
    // `y = x` references module-level `x`. The unqualified access's type
    // should equal x's declared type (Int).
    harness::with_snippet(&base, "x: Int = 1\ny = x", |ctx| {
        let access = harness::find_first(ctx.tree(pkl_types::FileId(1)).unwrap(), |k| {
            matches!(k, NodeKind::UnqualifiedAccessExpr)
        });
        let ty = pkl_types::expr_type(
            harness::expr(pkl_types::NodeRef::new(pkl_types::FileId(1), access), ctx),
            ctx,
        );
        assert!(
            ctx.base.int_type().is_subtype_of(&ty, ctx),
            "expected Int (or super), got {ty:?}"
        );
    });
}

#[test]
fn qualified_access_resolves_to_member_type() {
    let base = harness::load_test_base();
    // Property access on a typed receiver. `foo.x` should have type Int.
    let src = r#"
class Foo {
  x: Int
}
foo: Foo = new Foo { x = 1 }
y = foo.x
"#;
    harness::with_snippet(&base, src, |ctx| {
        let access = harness::find_first(ctx.tree(pkl_types::FileId(1)).unwrap(), |k| {
            matches!(k, NodeKind::QualifiedAccessExpr)
        });
        let ty = pkl_types::expr_type(
            harness::expr(pkl_types::NodeRef::new(pkl_types::FileId(1), access), ctx),
            ctx,
        );
        assert!(
            ctx.base.int_type().is_subtype_of(&ty, ctx),
            "expected Int (or super), got {ty:?}"
        );
    });
}

#[test]
fn let_binding_resolves_through_inner_function() {
    // `myVar` is bound by a let; the function literal introduces its own param
    // `y`. Looking up `myVar` from inside the function body requires walking
    // out past the function's ParameterList to the enclosing LetExpr.
    let base = harness::load_test_base();
    let src = "x = let (myVar: Int = 1) (y: Int) -> myVar + y";
    harness::with_snippet(&base, src, |ctx| {
        let my_var =
            find_unqualified_access_named(ctx.tree(pkl_types::FileId(1)).unwrap(), "myVar")
                .expect("expected an UnqualifiedAccessExpr named `myVar`");
        let ty = pkl_types::expr_type(
            harness::expr(pkl_types::NodeRef::new(pkl_types::FileId(1), my_var), ctx),
            ctx,
        );
        assert!(
            ctx.base.int_type().is_subtype_of(&ty, ctx),
            "expected Int (or super) for `myVar`, got {ty:?}"
        );
    });
}

fn find_unqualified_access_named(
    tree: &pkl_syntax::ast::SyntaxTree,
    target: &str,
) -> Option<pkl_syntax::ast::NodeId> {
    fn go(
        tree: &pkl_syntax::ast::SyntaxTree,
        id: pkl_syntax::ast::NodeId,
        target: &str,
    ) -> Option<pkl_syntax::ast::NodeId> {
        if matches!(tree.kind(id), NodeKind::UnqualifiedAccessExpr) && tree.name(id) == Some(target)
        {
            return Some(id);
        }
        for &child in tree.children(id) {
            if let Some(hit) = go(tree, child, target) {
                return Some(hit);
            }
        }
        None
    }
    go(tree, tree.root(), target)
}

#[test]
fn if_expr_unions_its_branches() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        r#"c = true
x = if (c) 1 else "a""#,
        |ctx| {
            let node = harness::find_first(ctx.tree(pkl_types::FileId(1)).unwrap(), |k| {
                matches!(k, NodeKind::IfExpr)
            });
            let ty = pkl_types::expr_type(
                harness::expr(pkl_types::NodeRef::new(pkl_types::FileId(1), node), ctx),
                ctx,
            );

            assert_eq!(
                ty,
                pkl_types::Type::union(
                    [
                        ctx.base.int_type().clone(),
                        pkl_types::Type::string_literal("a"),
                    ],
                    ctx
                )
            );
        },
    );
}

#[test]
fn access_null_safety_ignores_comments() {
    let base = harness::load_test_base();
    for (operator, nullable) in [
        ("/* receiver comment */ ?. /* member comment */", true),
        ("/* not an operator: ?. */ . /* member comment */", false),
    ] {
        let source =
            format!("class Foo {{ x: Int }}\nfoo: Foo = new Foo {{ x = 1 }}\ny = foo {operator} x");
        harness::with_snippet(&base, &source, |ctx| {
            let access = harness::find_first(ctx.tree(pkl_types::FileId(1)).unwrap(), |k| {
                matches!(k, NodeKind::QualifiedAccessExpr)
            });
            let ty = pkl_types::expr_type(
                harness::expr(pkl_types::NodeRef::new(pkl_types::FileId(1), access), ctx),
                ctx,
            );
            let expected = if nullable {
                ctx.base.int_type().nullable(ctx)
            } else {
                ctx.base.int_type().clone()
            };
            assert_eq!(ty, expected, "operator: {operator}");
        });
    }
}
