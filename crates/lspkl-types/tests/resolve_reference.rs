//! Local, base, and imported references.

mod harness;

use std::sync::Arc;

use lspkl_syntax::TextSize;
use lspkl_syntax::ast::{NodeId, NodeKind};
use lspkl_types::{AnalysisContext, NodeRef, resolver::resolve_reference};

/// Snippet:
///   x = 1
///   y = x
///         ^ cursor on the `x` reference in `y = x`
///
/// `resolve_reference` should return the `ClassProperty` node for `x = 1`.
#[test]
fn unqualified_access_to_local_property() {
    let base = harness::load_test_base();
    let src = "x = 1\ny = x\n";
    harness::with_snippet(&base, src, |ctx| {
        // Find the access expression `x` on line 2 (1-indexed: line=2).
        // Column 5 = the `x` in `y = x` (1-indexed: `y`=1, ` `=2, `=`=3, ` `=4, `x`=5).
        let cursor = ctx
            .tree(harness::file(ctx, 1))
            .unwrap()
            .find_at(TextSize::from(10))
            .expect("cursor lands on a node");
        let node = NodeRef::new(harness::file(ctx, 1), cursor);

        let resolved = resolve_reference(node, TextSize::from(10), ctx)
            .expect("`x` in `y = x` should resolve to the property declaration");

        assert_eq!(
            resolved.file,
            harness::file(ctx, 1),
            "local property should resolve within the same file",
        );
        assert!(
            matches!(
                ctx.tree(harness::file(ctx, 1))
                    .unwrap()
                    .kind(resolved.value),
                NodeKind::ClassProperty
            ),
            "expected ClassProperty, got {:?}",
            ctx.tree(harness::file(ctx, 1))
                .unwrap()
                .kind(resolved.value),
        );
        assert_eq!(
            ctx.tree(harness::file(ctx, 1))
                .unwrap()
                .name(resolved.value),
            Some("x")
        );
    });
}

/// Snippet:
///   x: String = ""
///      ^^^^^^ cursor anywhere on `String`
///
/// `String` is a declared type; resolution should cross over into `pkl:base`.
#[test]
fn declared_type_resolves_into_base() {
    let base = harness::load_test_base();
    let src = "x: String = \"\"\n";
    harness::with_snippet(&base, src, |ctx| {
        // Column 4 = the `S` of `String` (1-indexed: `x`=1, `:`=2, ` `=3, `S`=4).
        let cursor = ctx
            .tree(harness::file(ctx, 1))
            .unwrap()
            .find_at(TextSize::from(3))
            .expect("cursor lands on a node");
        let node = NodeRef::new(harness::file(ctx, 1), cursor);

        let resolved = resolve_reference(node, TextSize::from(3), ctx)
            .expect("`String` should resolve to the base-module class");

        assert_eq!(
            resolved.file,
            base.file_id(),
            "stdlib type should resolve into pkl:base",
        );
    });
}

/// Cursor on whitespace: nothing to resolve.
///
/// `find_at` will return the enclosing module/etc., and `resolve_reference`
/// on a non-reference node kind should return `None`.
#[test]
fn cursor_on_whitespace_returns_none() {
    let base = harness::load_test_base();
    let src = "x = 1\n";
    harness::with_snippet(&base, src, |ctx| {
        // Column 6 = the trailing newline area on line 1.
        let cursor = ctx
            .tree(harness::file(ctx, 1))
            .unwrap()
            .find_at(TextSize::from(5))
            .expect("cursor lands on a node");
        let node = NodeRef::new(harness::file(ctx, 1), cursor);

        assert!(
            resolve_reference(node, TextSize::from(5), ctx).is_none(),
            "no reference at this position; should return None",
        );
    });
}

fn with_import_context(f: impl FnOnce(&AnalysisContext)) {
    let base = harness::load_test_base();
    let current = Arc::new(
        lspkl_syntax::parse("import \"2.pkl\" as dep\nimport \"3.pkl\" as other\n").unwrap(),
    );
    let other = Arc::new(lspkl_syntax::parse("import \"4.pkl\" as dep\n").unwrap());
    base.add(1, current.source());
    base.add(2, "oneValue = 1\n");
    base.add(3, other.source());
    base.add(4, "twoValue = 2\n");
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, lspkl_db::Db::file(&*db, &harness::uri(1)).unwrap());
    f(&ctx);
}

#[test]
fn import_alias_in_an_imported_file_resolves_with_its_own_file_id() {
    with_import_context(|ctx| {
        let tree = ctx.tree(harness::file(ctx, 3)).unwrap();
        let import = harness::find_first(tree, |kind| matches!(kind, NodeKind::Import));
        let alias = tree.identifier(import).unwrap();
        let span = tree.span(alias);
        assert_eq!(
            resolve_reference(
                NodeRef::new(harness::file(ctx, 3), alias),
                span.start(),
                ctx
            ),
            Some(NodeRef::new(harness::file(ctx, 4), NodeId::ROOT)),
        );
    });
}

#[test]
fn import_uri_cursor_resolves_to_the_target_module_root() {
    with_import_context(|ctx| {
        let cursor = ctx
            .tree(harness::file(ctx, 1))
            .unwrap()
            .find_at(TextSize::from(9))
            .unwrap();
        assert_eq!(
            resolve_reference(
                NodeRef::new(harness::file(ctx, 1), cursor),
                TextSize::from(9),
                ctx
            ),
            Some(NodeRef::new(harness::file(ctx, 2), NodeId::ROOT)),
        );
    });
}

#[test]
fn import_declaration_does_not_resolve_outside_its_alias() {
    with_import_context(|ctx| {
        let import = harness::find_first(ctx.tree(harness::file(ctx, 1)).unwrap(), |kind| {
            matches!(kind, NodeKind::Import)
        });
        assert_eq!(
            resolve_reference(
                NodeRef::new(harness::file(ctx, 1), import),
                TextSize::from(0),
                ctx
            ),
            None
        );
    });
}

#[test]
fn unresolved_and_glob_imports_and_unrelated_strings_have_no_target() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "import \"missing.pkl\" as dep\nimport* \"*.pkl\" as all\nvalue = \"plain\"\n",
        |ctx| {
            for import in lspkl_syntax::ast::nodes::Module::cast(
                ctx.tree(harness::file(ctx, 1)).unwrap(),
                ctx.tree(harness::file(ctx, 1)).unwrap().root(),
            )
            .unwrap()
            .imports(ctx.tree(harness::file(ctx, 1)).unwrap())
            {
                let alias = ctx
                    .tree(harness::file(ctx, 1))
                    .unwrap()
                    .identifier(import)
                    .unwrap();
                let span = ctx.tree(harness::file(ctx, 1)).unwrap().span(alias);
                assert_eq!(
                    resolve_reference(
                        NodeRef::new(harness::file(ctx, 1), alias),
                        span.start(),
                        ctx
                    ),
                    None
                );
            }
            let offset = TextSize::try_from(
                ctx.tree(harness::file(ctx, 1))
                    .unwrap()
                    .source()
                    .rfind("plain")
                    .unwrap(),
            )
            .unwrap();
            let cursor = ctx
                .tree(harness::file(ctx, 1))
                .unwrap()
                .find_at(offset)
                .unwrap();
            assert_eq!(
                resolve_reference(NodeRef::new(harness::file(ctx, 1), cursor), offset, ctx),
                None
            );
        },
    );
}

#[test]
fn reference_after_unicode_resolves_at_its_inclusive_end() {
    let base = harness::load_test_base();
    let source = "x = 1\ntext = \"é😀\"; y = x";
    harness::with_snippet(&base, source, |ctx| {
        let offset = TextSize::try_from(source.len()).unwrap();
        let cursor = ctx
            .tree(harness::file(ctx, 1))
            .unwrap()
            .find_at(offset)
            .expect("reference includes EOF");
        let resolved =
            resolve_reference(NodeRef::new(harness::file(ctx, 1), cursor), offset, ctx).unwrap();
        assert_eq!(
            ctx.tree(harness::file(ctx, 1))
                .unwrap()
                .name(resolved.value),
            Some("x")
        );
    });
}

/// Resolve at the first byte of `token`'s `occurrence`th appearance in `src`.
fn resolve_at(ctx: &AnalysisContext, src: &str, token: &str, occurrence: usize) -> Option<NodeRef> {
    let offset = src
        .match_indices(token)
        .nth(occurrence)
        .unwrap_or_else(|| panic!("missing occurrence {occurrence} of {token:?}"))
        .0;
    let file = harness::file(ctx, 1);
    let offset = TextSize::from(offset as u32);
    let cursor = ctx.tree(file).unwrap().find_at(offset)?;
    resolve_reference(NodeRef::new(file, cursor), offset, ctx)
}

fn assert_declaration(
    ctx: &AnalysisContext,
    resolved: Option<NodeRef>,
    kind: NodeKind,
    name: &str,
) {
    let resolved = resolved.unwrap_or_else(|| panic!("`{name}` should resolve"));
    let tree = ctx.tree(resolved.file).unwrap();
    assert_eq!(resolved.file, harness::file(ctx, 1));
    assert_eq!(*tree.kind(resolved.value), kind);
    assert_eq!(tree.name(resolved.value), Some(name));
}

/// Module, class, and object property names resolve to their own declarations.
#[test]
fn property_declaration_names_resolve_to_themselves() {
    let base = harness::load_test_base();
    let src = "class Settings {\n  port: Int = 8080\n}\ncount = 1\nsettings: Settings = new { port = 9090 }\n";
    harness::with_snippet(&base, src, |ctx| {
        assert_declaration(
            ctx,
            resolve_at(ctx, src, "count", 0),
            NodeKind::ClassProperty,
            "count",
        );
        assert_declaration(
            ctx,
            resolve_at(ctx, src, "settings", 0),
            NodeKind::ClassProperty,
            "settings",
        );
        assert_declaration(
            ctx,
            resolve_at(ctx, src, "port", 0),
            NodeKind::ClassProperty,
            "port",
        );
        assert_declaration(
            ctx,
            resolve_at(ctx, src, "port", 1),
            NodeKind::ObjectProperty,
            "port",
        );
    });
}

/// Only the name resolves to the declaration; references inside the
/// initializer keep resolving through ordinary scope, and other positions in
/// the declaration resolve to nothing.
#[test]
fn property_self_resolution_is_limited_to_the_name() {
    let base = harness::load_test_base();
    let src = "other = 1\nlocal x: Int = other + 2\n";
    harness::with_snippet(&base, src, |ctx| {
        assert_declaration(
            ctx,
            resolve_at(ctx, src, "x", 0),
            NodeKind::ClassProperty,
            "x",
        );
        assert_declaration(
            ctx,
            resolve_at(ctx, src, "other", 1),
            NodeKind::ClassProperty,
            "other",
        );
        assert!(
            resolve_at(ctx, src, "local", 0).is_none(),
            "modifier is not the name"
        );
        assert!(
            resolve_at(ctx, src, "2", 0).is_none(),
            "initializer literal is not the name"
        );
    });
}
