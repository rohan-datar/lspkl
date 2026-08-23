//! Direct `render_markdown` tests.
//!
//! These bypass `resolve_reference` by pointing `render_markdown` straight at
//! a declaration node, so they pin the rendering format without depending on
//! resolution. The end-to-end path (cursor → resolve → render) is covered in
//! `tests/hover.rs`.

mod harness;

use pkl_syntax::ast::NodeKind;
use pkl_types::{FileId, NodeRef};

use lsp_features::hover::render_markdown;

/// Parse `source`, find the first top-level node matching `kind`, and return
/// `render_markdown` for it (with `original == resolved`).
fn render_first(source: &str, kind: NodeKind) -> Option<String> {
    let base = harness::load_test_base();
    harness::with_snippet(&base, source, |tree, ctx| {
        let node_id = *tree
            .children_matching(tree.root(), |k| *k == kind)
            .first()
            .unwrap_or_else(|| panic!("no top-level {kind:?} in snippet"));
        let node = NodeRef::new(FileId(1), node_id);
        render_markdown(node, node, ctx)
    })
}

#[test]
fn renders_annotated_class_property() {
    let md = render_first("x: String = \"hi\"\n", NodeKind::ClassProperty);
    assert_eq!(md.as_deref(), Some("```pkl\nx: String\n```"));
}

#[test]
fn renders_unannotated_class_property_with_inferred_type() {
    let md = render_first("x = 1\n", NodeKind::ClassProperty);
    assert_eq!(md.as_deref(), Some("```pkl\nx: Int\n```"));
}

#[test]
fn renders_class() {
    let md = render_first("class Foo {}\n", NodeKind::Class);
    assert_eq!(md.as_deref(), Some("```pkl\nclass Foo\n```"));
}

#[test]
fn renders_type_alias() {
    let md = render_first("typealias Bar = String\n", NodeKind::TypeAlias);
    assert_eq!(md.as_deref(), Some("```pkl\ntypealias Bar\n```"));
}

#[test]
fn renders_class_method() {
    let md = render_first("function baz(n: Int): Int = n\n", NodeKind::ClassMethod);
    assert_eq!(
        md.as_deref(),
        Some("```pkl\nfunction baz(n: Int): Int\n```")
    );
}

/// Full signature rendering: type parameters and parameter list with
/// per-parameter annotations.
#[test]
fn renders_method_with_type_parameters_and_params() {
    let md = render_first(
        "function map<A, B>(f: Function1<A, B>, x: A): B = f(x)\n",
        NodeKind::ClassMethod,
    );
    assert_eq!(
        md.as_deref(),
        Some("```pkl\nfunction map<A, B>(f: Function1<A, B>, x: A): B\n```")
    );
}

/// Without a declared return type the body expression is inferred
#[test]
fn renders_method_with_inferred_return_type() {
    // Body is a plain reference — binary expressions are avoided because
    // `compute_expr_type` panics on them (the parser drops operator
    // terminals; see the report on pkl-types gaps).
    let md = render_first("function baz(n: Int) = n\n", NodeKind::ClassMethod);
    assert_eq!(
        md.as_deref(),
        Some("```pkl\nfunction baz(n: Int): Int\n```")
    );
}

/// Unresolvable return types fall back to `unknown` (upstream behavior).
#[test]
fn renders_method_with_untyped_body_as_unknown() {
    let md = render_first("function boom(n: Int) = nope.nope\n", NodeKind::ClassMethod);
    assert!(md.as_deref().is_some_and(|m| m.contains("): unknown")));
}

/// Blank parameters render as `_`.
#[test]
fn renders_method_with_blank_parameter() {
    let md = render_first("function twice(_): Int = 2\n", NodeKind::ClassMethod);
    assert_eq!(md.as_deref(), Some("```pkl\nfunction twice(_): Int\n```"));
}

#[test]
fn renders_property_modifiers() {
    let md = render_first("hidden fixed x: Int = 1\n", NodeKind::ClassProperty);
    assert_eq!(md.as_deref(), Some("```pkl\nhidden fixed x: Int\n```"));
}

#[test]
fn renders_class_modifiers() {
    let md = render_first("abstract open class Foo {}\n", NodeKind::Class);
    assert_eq!(md.as_deref(), Some("```pkl\nabstract open class Foo\n```"));
}

/// Doc comments render after the fenced block, separated by a `---`
/// rule (mirroring `showDocCommentAndModule`).
#[test]
fn renders_doc_comment_after_signature_block() {
    let md = render_first(
        "/// Docs for x.\n/// More detail.\nx: Int = 1\n",
        NodeKind::ClassProperty,
    );
    assert_eq!(
        md.as_deref(),
        Some("```pkl\nx: Int\n```\n\n---\n\nDocs for x.\nMore detail.")
    );
}

/// Method modifiers combine with the full signature.
#[test]
fn renders_method_modifiers_with_signature() {
    let md = render_first(
        "hidden function baz(n: Int): Int = n\n",
        NodeKind::ClassMethod,
    );
    assert_eq!(
        md.as_deref(),
        Some("```pkl\nhidden function baz(n: Int): Int\n```")
    );
}

#[test]
fn returns_none_for_non_declarations() {
    let base = harness::load_test_base();
    harness::with_snippet(&base, "x = 1\n", |tree, ctx| {
        // The module root itself is not a declaration we render.
        let node = NodeRef::new(FileId(1), tree.root());
        assert_eq!(render_markdown(node, node, ctx), None);
    });
}
