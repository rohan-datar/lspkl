//! `compute_this_type` tests.

mod harness;

use lspkl_syntax::ast::NodeKind;
use lspkl_types::Type;

#[test]
fn this_type_at_module_top_is_module() {
    let base = harness::load_test_base();
    // Module-level property; the `1` literal lives directly under Module.
    harness::with_snippet(&base, "x = 1", |ctx| {
        let lit = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |k| {
            matches!(k, NodeKind::IntLiteralExpr)
        });
        let ty = lspkl_types::this_type(
            ctx.db,
            lspkl_types::NodeRef::new(harness::file(ctx, 1), lit),
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
        let ty = lspkl_types::this_type(
            ctx.db,
            lspkl_types::NodeRef::new(harness::file(ctx, 1), lit),
            ctx.root,
        );

        // Should be Type::Class pointing at Foo.
        let Type::Class { class_id, .. } = &ty else {
            panic!("expected Type::Class, got {ty:?}");
        };

        // Verify it's actually pointing at `Foo` by checking the name.
        let items = lspkl_types::module_items(ctx.db, class_id.module(ctx.db).file(ctx.db));
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
        let ty = lspkl_types::this_type(
            ctx.db,
            lspkl_types::NodeRef::new(harness::file(ctx, 1), inner),
            ctx.root,
        );

        // Should be Type::Class pointing at Int (which lives in pkl:base).
        assert!(
            ty.class_equals(ctx.base.int_type()),
            "expected Type::Class for Int, got {ty:?}"
        );
    });
}

/// Assert each `this` independently, including transitions back out of bodies.
fn assert_receivers(source: &str, expected: &[&str]) {
    let base = harness::load_test_base();
    harness::with_snippet(&base, source, |ctx| {
        let file = harness::file(ctx, 1);
        let tree = ctx.tree(file).unwrap();
        fn visit(
            tree: &lspkl_syntax::ast::SyntaxTree,
            id: lspkl_syntax::ast::NodeId,
            result: &mut Vec<lspkl_syntax::ast::NodeId>,
        ) {
            if matches!(tree.kind(id), NodeKind::ThisExpr) {
                result.push(id);
            }
            for child in tree.children(id) {
                visit(tree, *child, result);
            }
        }
        let mut nodes = Vec::new();
        visit(tree, tree.root(), &mut nodes);
        let actual: Vec<_> = nodes
            .into_iter()
            .map(|id| {
                lspkl_types::this_type(ctx.db, lspkl_types::NodeRef::new(file, id), ctx.root)
                    .render(ctx.db)
            })
            .collect();
        assert_eq!(actual, expected, "{source}");
    });
}

#[test]
fn explicit_and_contextual_new_receivers_keep_type_arguments() {
    assert_receivers(
        r#"
x = new Mapping<String, Int> { local probe = this }
y: Listing<String> = new { local probe = this }
"#,
        &["Mapping<String, Int>", "Listing<String>"],
    );
}

#[test]
fn nested_property_amendments_change_receiver() {
    assert_receivers(
        r#"
class Inner { number: Int }
class Outer { inner: Inner }
x = new Outer {
  local outerProbe = this
  inner { local nested = this }
  local back = this
}
"#,
        &["Outer", "Inner", "Outer"],
    );
}

#[test]
fn contextual_new_in_calls_and_container_values() {
    assert_receivers(
        r#"
class Inner { number: Int }
function accept<T>(value: Listing<T>): Listing<T> = value
x = accept<String>(new { local probe = this })
y: Listing<Inner> = new { new { local probe = this } }
z: Mapping<String, Inner> = new { ["key"] = new { local probe = this } }
"#,
        &["Listing<String>", "Inner", "Inner"],
    );
}

#[test]
fn amendment_parent_uses_outer_this_but_body_uses_receiver() {
    assert_receivers(
        r#"
class Inner { number: Int }
class Outer { inner: Inner }
x = new Outer {
  local value = (this.inner) { local probe = this }
}
"#,
        &["Outer", "Inner"],
    );
}

#[test]
fn generator_bodies_preserve_receiver() {
    assert_receivers(
        r#"
x = new Listing<Int> {
  when (true) { local probe = this }
  for (n in List(1)) { local probe = this }
}
"#,
        &["Listing<Int>", "Listing<Int>"],
    );
}

#[test]
fn nested_output_files_and_entry_amendments() {
    assert_receivers(
        r#"
output { files { ["hello.txt"] { local probe = this } } }
"#,
        &["FileOutput"],
    );
}

#[test]
fn annotation_body_uses_annotation_class() {
    assert_receivers(
        "class Ann extends Annotation { value: String }\n@Ann { local probe = this }\nx = 1",
        &["Ann"],
    );
}

#[test]
fn property_bodies_keep_inferred_and_generic_member_types() {
    assert_receivers(
        r#"
class Inner { number: Int }
class Inferred { inner = new Inner {} }
class Box<T> { child: T }
x = new Inferred { inner { local probe = this } }
y = new Box<Inner> { child { local probe = this } }
z = new Listing<Listing<Int>> { new { local probe = this } }
"#,
        &["Inner", "Inner", "Listing<Int>"],
    );
}

#[test]
fn inherited_amendments_retain_property_receiver_type() {
    assert_receivers(
        r#"
class Inner { number: Int }
open class Parent { child: Inner }
class Child extends Parent { child {} }
x = new Child { child { local probe = this } }
"#,
        &["Inner"],
    );
}

#[test]
fn guarded_member_receivers_narrow_only_in_their_branch() {
    let base = harness::load_test_base();
    let source = r#"
open class Parent {}
class Child extends Parent { number: Int }
value: Parent = new Child {}
x = if (value is Child) value.number else 0
"#;
    harness::with_snippet(&base, source, |ctx| {
        let file = harness::file(ctx, 1);
        let node = harness::find_first(ctx.tree(file).unwrap(), |k| {
            matches!(k, NodeKind::QualifiedAccessExpr)
        });
        let ty = lspkl_types::expr_type(
            ctx.db,
            harness::expr(lspkl_types::NodeRef::new(file, node), ctx),
            ctx.root,
        );
        assert!(ty.class_equals(ctx.base.int_type()), "{ty:?}");
    });
}
