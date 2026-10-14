//! `expected_type` tests for types required by an expression's surrounding context.

mod harness;

use lspkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};
use lspkl_types::Type;

/// Find the *nth* node whose kind satisfies `predicate`. Useful when a snippet
/// has multiple nodes of the same kind (e.g. two `IntLiteralExpr`s).
fn find_nth<F: Fn(&NodeKind) -> bool>(tree: &SyntaxTree, n: usize, predicate: F) -> NodeId {
    fn go<F: Fn(&NodeKind) -> bool>(
        tree: &SyntaxTree,
        id: NodeId,
        predicate: &F,
        seen: &mut usize,
        target: usize,
    ) -> Option<NodeId> {
        if predicate(tree.kind(id)) {
            if *seen == target {
                return Some(id);
            }
            *seen += 1;
        }
        for &child in tree.children(id) {
            if let Some(hit) = go(tree, child, predicate, seen, target) {
                return Some(hit);
            }
        }
        None
    }
    let mut seen = 0;
    go(tree, tree.root(), &predicate, &mut seen, n).expect("nth matching node not found")
}

// ---------------------------------------------------------------------------
// Fallback (parent_declared_type) — the most-used case
// ---------------------------------------------------------------------------

#[test]
fn class_property_rhs_inherits_declared_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x: Int = 1", |ctx| {
        let lit = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::IntLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), lit), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.int_type()),
            "expected Int, got {ty:?}"
        );
    });
}

#[test]
fn class_property_rhs_without_annotation_is_unknown() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = 1", |ctx| {
        let lit = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::IntLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), lit), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert_eq!(ty, Type::Unknown);
    });
}

#[test]
fn class_method_body_inherits_return_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "function f(): Int = 1", |ctx| {
        let lit = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::IntLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), lit), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.int_type()),
            "expected Int, got {ty:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// Control flow / structural
// ---------------------------------------------------------------------------

#[test]
fn if_expr_condition_is_boolean() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = if (true) 1 else 2", |ctx| {
        let cond = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::TrueLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), cond), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.boolean_type()),
            "expected Boolean, got {ty:?}"
        );
    });
}

#[test]
fn if_expr_branch_recurses_to_outer_context() {
    let base = harness::load_test_base();
    // The Int literal `1` is the then-branch of an IfExpr that's the RHS of
    // a typed property — so the branch should expect Int.
    harness::with_snippet(&base, "x: Int = if (true) 1 else 2", |ctx| {
        let lit = find_nth(ctx.tree(harness::file(ctx, 1)).unwrap(), 0, |k| {
            matches!(k, NodeKind::IntLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), lit), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.int_type()),
            "expected Int (recursed from outer), got {ty:?}"
        );
    });
}

#[test]
fn let_expr_init_uses_binding_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = let (v: Int = 1) v", |ctx| {
        let lit = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::IntLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), lit), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.int_type()),
            "expected Int (binding's declared type), got {ty:?}"
        );
    });
}

#[test]
fn let_expr_body_recurses_to_outer_context() {
    let base = harness::load_test_base();
    // The body `v` should inherit the property's `Int` annotation.
    harness::with_snippet(&base, "x: Int = let (v: Int = 1) v", |ctx| {
        let body = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::UnqualifiedAccessExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), body), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.int_type()),
            "expected Int (recursed from outer), got {ty:?}"
        );
    });
}

#[test]
fn parenthesized_expr_is_transparent() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x: Int = (1)", |ctx| {
        let lit = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::IntLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), lit), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.int_type()),
            "expected Int (transparent through parens), got {ty:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// Builtins with String arguments
// ---------------------------------------------------------------------------

#[test]
fn read_expr_arg_is_string() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, r#"x = read("foo")"#, |ctx| {
        let s = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::SingleLineStringLiteral)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), s), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.string_type()),
            "expected String, got {ty:?}"
        );
    });
}

#[test]
fn throw_expr_arg_is_string() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, r#"x = throw("oops")"#, |ctx| {
        let s = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::SingleLineStringLiteral)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), s), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.string_type()),
            "expected String, got {ty:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// Unary operators
// ---------------------------------------------------------------------------

#[test]
fn unary_minus_operand_accepts_number_duration_and_data_size() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = -1", |ctx| {
        let lit = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::IntLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), lit), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        for accepted in [
            ctx.base.int_type(),
            ctx.base.float_type(),
            ctx.base.duration_type(),
            ctx.base.data_size_type(),
        ] {
            assert!(
                accepted.is_subtype_of(&ty, ctx.db),
                "expected {accepted:?} to satisfy {ty:?}"
            );
        }
        assert!(
            !ctx.base.string_type().is_subtype_of(&ty, ctx.db),
            "String must not satisfy {ty:?}"
        );
    });
}

#[test]
fn logical_not_operand_is_boolean() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = !true", |ctx| {
        let lit = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::TrueLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), lit), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.boolean_type()),
            "expected Boolean, got {ty:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// Binary operators
// ---------------------------------------------------------------------------

#[test]
fn logical_and_operands_are_boolean() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = true && false", |ctx| {
        let lhs = find_nth(ctx.tree(harness::file(ctx, 1)).unwrap(), 0, |k| {
            matches!(k, NodeKind::TrueLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), lhs), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.boolean_type()),
            "expected Boolean, got {ty:?}"
        );
    });
}

#[test]
fn logical_or_operands_are_boolean() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = true || false", |ctx| {
        let lhs = find_nth(ctx.tree(harness::file(ctx, 1)).unwrap(), 0, |k| {
            matches!(k, NodeKind::TrueLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), lhs), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.boolean_type()),
            "expected Boolean, got {ty:?}"
        );
    });
}

#[test]
fn exponentiation_operands_are_number() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = 2 ** 3", |ctx| {
        let lhs = find_nth(ctx.tree(harness::file(ctx, 1)).unwrap(), 0, |k| {
            matches!(k, NodeKind::IntLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), lhs), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.number_type()),
            "expected Number, got {ty:?}"
        );
    });
}

#[test]
fn additive_operand_expects_other_operands_type() {
    let base = harness::load_test_base();
    // Left operand expects right operand's type (Int).
    harness::with_snippet(&base, "x = 1 + 2", |ctx| {
        let lhs = find_nth(ctx.tree(harness::file(ctx, 1)).unwrap(), 0, |k| {
            matches!(k, NodeKind::IntLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), lhs), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.int_type()),
            "expected Int (mirror other operand), got {ty:?}"
        );
    });
}

#[test]
fn multiplicative_default_is_number() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = 2 * 3", |ctx| {
        let lhs = find_nth(ctx.tree(harness::file(ctx, 1)).unwrap(), 0, |k| {
            matches!(k, NodeKind::IntLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), lhs), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.number_type()),
            "expected Number, got {ty:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// For-generator iterable (works without compute_this_type — fixed return)
// ---------------------------------------------------------------------------

#[test]
fn for_generator_iterable_is_collection() {
    let base = harness::load_test_base();
    // Use an identifier rather than a range literal — tree-sitter-pkl
    // currently parses `..` with an Error node, which is irrelevant to this
    // test but would confuse find_first.
    harness::with_snippet(&base, "x = new { for (i in items) {} }", |ctx| {
        let iter = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::UnqualifiedAccessExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), iter), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.collection_type()),
            "expected Collection, got {ty:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// Container value types require compute_this_type_inner to resolve object bodies.
// It currently returns Unknown for NewExpr, AmendExpr, and ObjectBody, so the
// expected value type cannot yet be extracted.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "compute_this_type_inner returns Unknown for NewExpr/ObjectBody receivers"]
fn object_entry_value_uses_mapping_value_type() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        r#"x: Mapping<String, Int> = new { ["k"] = 1 }"#,
        |ctx| {
            // Find the value side of the ObjectEntry (the `1`).
            let val = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
                matches!(k, NodeKind::IntLiteralExpr)
            });
            let ty = lspkl_types::expected_type(
                ctx.db,
                harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), val), ctx)
                    .map(|expr| expr.id()),
                ctx.root,
            );
            assert!(
                ty.class_equals(ctx.base.int_type()),
                "expected Int, got {ty:?}"
            );
        },
    );
}

#[test]
#[ignore = "compute_this_type_inner returns Unknown for NewExpr/ObjectBody receivers"]
fn member_predicate_value_uses_container_value_type() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, r#"x: Listing<Int> = new { [[true]] = 1 }"#, |ctx| {
        let val = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::IntLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), val), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.int_type()),
            "expected Int, got {ty:?}"
        );
    });
}

#[test]
fn member_predicate_condition_is_boolean() {
    let base = harness::load_test_base();
    // Condition position doesn't need compute_this_type — it's a fixed Boolean.
    harness::with_snippet(&base, r#"x: Listing<Int> = new { [[true]] = 1 }"#, |ctx| {
        let cond = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::TrueLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), cond), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.boolean_type()),
            "expected Boolean, got {ty:?}"
        );
    });
}

#[test]
fn when_generator_condition_is_boolean() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = new { when (true) { 1 } }", |ctx| {
        let cond = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::TrueLiteralExpr)
        });
        let ty = lspkl_types::expected_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(harness::file(ctx, 1), cond), ctx)
                .map(|expr| expr.id()),
            ctx.root,
        );
        assert!(
            ty.class_equals(ctx.base.boolean_type()),
            "expected Boolean, got {ty:?}"
        );
    });
}
