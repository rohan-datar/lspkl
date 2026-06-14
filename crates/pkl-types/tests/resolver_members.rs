//! Member-resolution tests focused on class inheritance.
//!
//! These are regression tests for inherited member lookup through `extends`.

mod harness;

use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};
use pkl_types::resolver::{find_member_on_type, resolve_declared_type, resolve_unqualified_access};
use pkl_types::{NodeRef, Type, TypeBindings};

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
        let my_person = find_class_named(ctx.tree, "MyPerson").expect("expected class MyPerson");
        let my_person_ty = Type::class(NodeRef::new(ctx.file_id, my_person));

        let resolved = find_member_on_type(&my_person_ty, "age", &TypeBindings::new(), ctx)
            .expect("expected to resolve inherited property `age`");

        assert!(
            ctx.base.int_type().is_subtype_of(&resolved.ty, ctx),
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
        let age_access = find_unqualified_access_named(ctx.tree, "age")
            .expect("expected unqualified access `age` in subclass body");

        let resolved = resolve_unqualified_access(age_access, "age", &TypeBindings::new(), ctx)
            .expect("expected unqualified `age` to resolve via inherited member lookup");

        assert!(
            ctx.base.int_type().is_subtype_of(&resolved.ty, ctx),
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
        let inner_t = find_declared_type_named(ctx.tree, "T")
            .expect("expected a DeclaredType named `T` inside the typealias body");

        let resolved = resolve_declared_type(ctx, NodeRef::new(ctx.file_id, inner_t))
            .expect("expected `T` to resolve via the enclosing typealias's TypeParameterList");

        // The resolved NodeRef should point at a TypeParameter node — that's
        // the binding site for `T` on the typealias.
        let target_tree = ctx
            .tree_for(resolved.file_id)
            .expect("resolved NodeRef should be in a known file");
        assert!(
            matches!(target_tree.kind(resolved.node_id), NodeKind::TypeParameter),
            "expected resolved kind TypeParameter, got {:?}",
            target_tree.kind(resolved.node_id),
        );
        assert_eq!(
            target_tree.name(resolved.node_id),
            Some("T"),
            "resolved TypeParameter should be named `T`",
        );
    });
}

fn find_declared_type_named(tree: &SyntaxTree, target: &str) -> Option<NodeId> {
    fn go(tree: &SyntaxTree, id: NodeId, target: &str) -> Option<NodeId> {
        if matches!(tree.kind(id), NodeKind::DeclaredType) {
            let segments = tree.qualified_identifier_segments(
                tree.child(id, |k| matches!(k, NodeKind::QualifiedIdentifier))?,
            );
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
