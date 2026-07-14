//! `resolve_reference` tests.
//!
//! These will panic on `todo!()` until you port the dispatch from
//! `PklNode.resolveReference` (Extensions.kt:400). The first to pass is
//! likely `unqualified_access_to_local_property` since it's the smallest
//! end-to-end loop (scope lookup of one identifier in one module).

mod harness;

use pkl_syntax::ast::NodeKind;
use pkl_types::{FileId, NodeRef, resolver::resolve_reference};

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
        let cursor = ctx.tree.find_at(2, 5).expect("cursor lands on a node");
        let node = NodeRef::new(ctx.file_id, cursor);

        let resolved = resolve_reference(node, 2, 5, ctx)
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
        let cursor = ctx.tree.find_at(1, 4).expect("cursor lands on a node");
        let node = NodeRef::new(ctx.file_id, cursor);

        let resolved = resolve_reference(node, 1, 4, ctx)
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
        let cursor = ctx.tree.find_at(1, 6).expect("cursor lands on a node");
        let node = NodeRef::new(ctx.file_id, cursor);

        assert!(
            resolve_reference(node, 1, 6, ctx).is_none(),
            "no reference at this position; should return None",
        );
    });
}
