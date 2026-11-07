mod harness;

use lsp_types::HoverContents;

#[test]
fn inferred_metadata_values_render_in_hover() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "class Foo {}\ntypealias Alias = Foo\na = Foo\nb = Alias\nuseClass = a\nuseAlias = b\n",
        |_tree, ctx| {
            for (line, expected) in [(4, "Class<Foo>"), (5, "TypeAlias")] {
                let file = harness::file(ctx, 1);
                let offset = harness::offset(ctx.tree(file).unwrap(), line, 11);
                let result = lspkl_features::hover::hover(
                    ctx.db,
                    ctx.root,
                    lspkl_types::InFile::new(file, offset),
                )
                .unwrap();
                let HoverContents::Markup(markup) = result.contents else {
                    panic!("expected markdown")
                };
                assert!(markup.value.contains(expected), "{}", markup.value);
            }
        },
    );
}
