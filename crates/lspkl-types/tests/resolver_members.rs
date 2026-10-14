//! Member-resolution tests focused on class inheritance.
//!
//! These are regression tests for inherited member lookup through `extends`.

mod harness;

use lspkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};
use lspkl_types::resolver::{resolve_declared_type, resolve_unqualified_access};
use lspkl_types::{NodeRef, Type};

#[test]
fn find_member_on_type_finds_property_on_superclass() {
    let base = harness::load_test_base();
    let src = r#"
open class Person {
  age: Int
}

class MyPerson extends Person {
  name: String
}
"#;

    harness::with_snippet(&base, src, |ctx| {
        let my_person = find_class_named(ctx.tree(harness::file(ctx, 1)).unwrap(), "MyPerson")
            .expect("expected class MyPerson");
        let my_person_ty = Type::class(
            lspkl_types::class_id(
                ctx.db,
                NodeRef::new(harness::file(ctx, 1), my_person),
                ctx.root,
            )
            .expect("class declaration"),
        );

        let resolved = lspkl_types::find_member(ctx.db, &my_person_ty, "age")
            .expect("expected to resolve inherited property `age`");

        assert!(
            ctx.base.int_type().is_subtype_of(&resolved.ty, ctx.db),
            "expected inherited property type Int (or super), got {:?}",
            resolved.ty
        );
    });
}

#[test]
fn resolve_unqualified_access_finds_inherited_property_in_subclass_body() {
    let base = harness::load_test_base();
    let src = r#"
open class Person {
  age: Int
}

class MyPerson extends Person {
  olderAge = age + 1
}
"#;

    harness::with_snippet(&base, src, |ctx| {
        let age_access =
            find_unqualified_access_named(ctx.tree(harness::file(ctx, 1)).unwrap(), "age")
                .expect("expected unqualified access `age` in subclass body");

        let resolved = resolve_unqualified_access(
            lspkl_types::NodeRef::new(harness::file(ctx, 1), age_access),
            "age",
            ctx,
        )
        .expect("expected unqualified `age` to resolve via inherited member lookup");

        assert!(
            ctx.base.int_type().is_subtype_of(&resolved.ty, ctx.db),
            "expected unqualified `age` type Int (or super), got {:?}",
            resolved.ty
        );
    });
}

#[test]
fn type_parameter_resolves_inside_typealias_body() {
    let base = harness::load_test_base();
    // `T` appears as a type-name reference inside the typealias body. It
    // should resolve to the `TypeParameter` declaration on the enclosing
    // typealias, NOT fall through to module/base lookup.
    let src = "typealias MyAlias<T> = Listing<T>";

    harness::with_snippet(&base, src, |ctx| {
        // Find the inner `T` — the DeclaredType inside Listing's TypeArgumentList.
        let inner_t = find_declared_type_named(ctx.tree(harness::file(ctx, 1)).unwrap(), "T")
            .expect("expected a DeclaredType named `T` inside the typealias body");

        let resolved = resolve_declared_type(ctx, NodeRef::new(harness::file(ctx, 1), inner_t))
            .expect("expected `T` to resolve via the enclosing typealias's TypeParameterList");

        // The resolved NodeRef should point at a TypeParameter node — that's
        // the binding site for `T` on the typealias.
        let target_tree = ctx
            .tree(resolved.file)
            .expect("resolved NodeRef should be in a known file");
        assert!(
            matches!(target_tree.kind(resolved.value), NodeKind::TypeParameter),
            "expected resolved kind TypeParameter, got {:?}",
            target_tree.kind(resolved.value),
        );
        assert_eq!(
            target_tree.name(resolved.value),
            Some("T"),
            "resolved TypeParameter should be named `T`",
        );
    });
}

fn find_declared_type_named(tree: &SyntaxTree, target: &str) -> Option<NodeId> {
    fn go(tree: &SyntaxTree, id: NodeId, target: &str) -> Option<NodeId> {
        if matches!(tree.kind(id), NodeKind::DeclaredType) {
            let segments = tree.declared_type_parts(id);
            if segments.last().copied() == Some(target) && segments.len() == 1 {
                return Some(id);
            }
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

fn find_class_named(tree: &SyntaxTree, target: &str) -> Option<NodeId> {
    fn go(tree: &SyntaxTree, id: NodeId, target: &str) -> Option<NodeId> {
        if matches!(tree.kind(id), NodeKind::Class) && tree.name(id) == Some(target) {
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

fn find_unqualified_access_named(tree: &SyntaxTree, target: &str) -> Option<NodeId> {
    fn go(tree: &SyntaxTree, id: NodeId, target: &str) -> Option<NodeId> {
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
