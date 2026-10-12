//! Local, base, and imported references.

mod harness;

use std::collections::HashMap;
use std::sync::Arc;

use pkl_syntax::TextSize;
use pkl_syntax::ast::{NodeId, NodeKind};
use pkl_types::{AnalysisContext, FileId, NodeRef, resolver::resolve_reference};

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
            .tree
            .find_at(TextSize::from(10))
            .expect("cursor lands on a node");
        let node = NodeRef::new(ctx.file_id, cursor);

        let resolved = resolve_reference(node, TextSize::from(10), ctx)
            .expect("`x` in `y = x` should resolve to the property declaration");

        assert_eq!(
            resolved.file_id, ctx.file_id,
            "local property should resolve within the same file",
        );
        assert!(
            matches!(ctx.tree.kind(resolved.node_id), NodeKind::ClassProperty),
            "expected ClassProperty, got {:?}",
            ctx.tree.kind(resolved.node_id),
        );
        assert_eq!(ctx.tree.name(resolved.node_id), Some("x"));
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
            .tree
            .find_at(TextSize::from(3))
            .expect("cursor lands on a node");
        let node = NodeRef::new(ctx.file_id, cursor);

        let resolved = resolve_reference(node, TextSize::from(3), ctx)
            .expect("`String` should resolve to the base-module class");

        assert_eq!(
            resolved.file_id,
            FileId::BASE,
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
            .tree
            .find_at(TextSize::from(5))
            .expect("cursor lands on a node");
        let node = NodeRef::new(ctx.file_id, cursor);

        assert!(
            resolve_reference(node, TextSize::from(5), ctx).is_none(),
            "no reference at this position; should return None",
        );
    });
}

fn with_import_context(f: impl FnOnce(&AnalysisContext)) {
    let base = harness::load_test_base();
    let current = Arc::new(pkl_syntax::parse("import \"one.pkl\" as dep\n").unwrap());
    let other = Arc::new(pkl_syntax::parse("import \"two.pkl\" as dep\n").unwrap());
    let current_import = harness::find_first(&current, |kind| matches!(kind, NodeKind::Import));
    let other_import = harness::find_first(&other, |kind| matches!(kind, NodeKind::Import));
    let trees = HashMap::from([
        (FileId(1), current.clone()),
        (
            FileId(2),
            Arc::new(pkl_syntax::parse("oneValue = 1\n").unwrap()),
        ),
        (FileId(3), other),
        (
            FileId(4),
            Arc::new(pkl_syntax::parse("twoValue = 2\n").unwrap()),
        ),
    ]);
    let imports = HashMap::from([
        (
            NodeRef::new(FileId(1), current_import),
            pkl_types::ImportResolution::Single(NodeRef::new(FileId(2), NodeId::ROOT)),
        ),
        (
            NodeRef::new(FileId(3), other_import),
            pkl_types::ImportResolution::Single(NodeRef::new(FileId(4), NodeId::ROOT)),
        ),
    ]);
    let ctx = AnalysisContext::new(&current, FileId(1), &base, &trees, &imports);
    f(&ctx);
}

#[test]
fn import_alias_in_an_imported_file_resolves_with_its_own_file_id() {
    with_import_context(|ctx| {
        let tree = ctx.tree_for(FileId(3)).unwrap();
        let import = harness::find_first(tree, |kind| matches!(kind, NodeKind::Import));
        let alias = tree.identifier(import).unwrap();
        let span = tree.span(alias);
        assert_eq!(
            resolve_reference(NodeRef::new(FileId(3), alias), span.start(), ctx),
            Some(NodeRef::new(FileId(4), NodeId::ROOT)),
        );
    });
}

#[test]
fn import_uri_cursor_resolves_to_the_target_module_root() {
    with_import_context(|ctx| {
        let cursor = ctx.tree.find_at(TextSize::from(9)).unwrap();
        assert_eq!(
            resolve_reference(NodeRef::new(ctx.file_id, cursor), TextSize::from(9), ctx),
            Some(NodeRef::new(FileId(2), NodeId::ROOT)),
        );
    });
}

#[test]
fn import_declaration_does_not_resolve_outside_its_alias() {
    with_import_context(|ctx| {
        let import = harness::find_first(ctx.tree, |kind| matches!(kind, NodeKind::Import));
        assert_eq!(
            resolve_reference(NodeRef::new(ctx.file_id, import), TextSize::from(0), ctx),
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
            for &import in
                ctx.tree.children(ctx.tree.root()).iter().filter(|&&id| {
                    matches!(ctx.tree.kind(id), NodeKind::Import | NodeKind::ImportGlob)
                })
            {
                let alias = ctx.tree.identifier(import).unwrap();
                let span = ctx.tree.span(alias);
                assert_eq!(
                    resolve_reference(NodeRef::new(ctx.file_id, alias), span.start(), ctx),
                    None
                );
            }
            let offset = TextSize::try_from(ctx.tree.source().rfind("plain").unwrap()).unwrap();
            let cursor = ctx.tree.find_at(offset).unwrap();
            assert_eq!(
                resolve_reference(NodeRef::new(ctx.file_id, cursor), offset, ctx),
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
        let cursor = ctx.tree.find_at(offset).expect("reference includes EOF");
        let resolved = resolve_reference(NodeRef::new(ctx.file_id, cursor), offset, ctx).unwrap();
        assert_eq!(ctx.tree.name(resolved.node_id), Some("x"));
    });
}
