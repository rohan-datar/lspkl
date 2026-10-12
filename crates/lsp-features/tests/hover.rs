//! End-to-end `hover` tests.
//!
//! Exercises the whole pipeline: `find_at` → `resolve_reference` →
//! `render_markdown` → `Hover` wrap. Each test will panic on `todo!()` in
//! the helper it reaches first; expect `resolve_reference` to be the first
//! to fall, then `render_markdown`.

mod harness;

use tower_lsp_server::lsp_types::{HoverContents, MarkupKind};

use lsp_features::hover::hover;

/// Snippet (positions are 0-indexed because LSP):
///   line 0: x = 1
///   line 1: y = x
///                ^ cursor on the `x` reference (line=1, character=4)
///
/// Hovering on the `x` reference should produce some markdown describing the
/// `x = 1` declaration. We don't pin the exact string here — that's a
/// rendering policy decision the user will make in `render_markdown`. We
/// just assert:
///   1. The result is `Some(Hover { .. })`
///   2. Contents are `MarkupKind::Markdown`
///   3. The rendered text mentions the property name `x`
#[test]
fn hover_on_unqualified_access_renders_property() {
    let base = harness::load_test_base();
    let src = "x = 1\ny = x\n";
    harness::with_snippet(&base, src, |_tree, ctx| {
        let offset = harness::offset(ctx.tree, 1, 4);

        let result = hover(ctx, offset).expect("hover should produce a result for `x`");

        let HoverContents::Markup(markup) = result.contents else {
            panic!("expected MarkupContent, got something else");
        };
        assert_eq!(markup.kind, MarkupKind::Markdown);
        assert!(
            markup.value.contains("x"),
            "rendered hover should mention the property name; got: {}",
            markup.value,
        );
    });
}

/// Cursor on whitespace at end of a line. Expect `None` — nothing to hover.
#[test]
fn hover_on_whitespace_returns_none() {
    let base = harness::load_test_base();
    let src = "x = 1\n";
    harness::with_snippet(&base, src, |_tree, ctx| {
        // Column 10 is well past `x = 1` (length 5).
        let offset = harness::offset(ctx.tree, 0, 10);

        assert!(
            hover(ctx, offset).is_none(),
            "no hover info expected past end of line",
        );
    });
}

/// End-to-end: hovering a reference surfaces the target's doc comment
/// after the fenced signature block (`---` rule framing, mirroring
/// showDocCommentAndModule).
#[test]
fn hover_includes_doc_comment_and_signature() {
    let base = harness::load_test_base();
    let src = "/// Doubles a number.\nfunction double(n: Int): Int = n\ny = double(1)\n";
    harness::with_snippet(&base, src, |_tree, ctx| {
        // Cursor on the `double` reference in `y = double(1)`.
        let offset = harness::offset(ctx.tree, 2, 6);

        let result = hover(ctx, offset).expect("hover should resolve `double`");
        let HoverContents::Markup(markup) = result.contents else {
            panic!("expected MarkupContent, got something else");
        };
        assert!(
            markup.value.contains("function double(n: Int): Int"),
            "expected full method signature; got: {}",
            markup.value,
        );
        assert!(
            markup.value.contains("---\n\nDoubles a number."),
            "expected doc comment after a --- rule; got: {}",
            markup.value,
        );
    });
}
