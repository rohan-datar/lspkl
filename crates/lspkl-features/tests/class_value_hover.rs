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

#[test]
fn implicit_typed_members_are_available_in_completion_and_hover() {
    let base = harness::load_test_base();
    let source = "class Foo {}\nvalue = new Foo {}\nresult = value.toMap()\n";
    harness::with_snippet(&base, source, |tree, ctx| {
        let offset = lspkl_syntax::TextSize::try_from(source.find("toMap").unwrap() + 2).unwrap();
        let position = lspkl_types::InFile::new(harness::file(ctx, 1), offset);
        let Some(lsp_types::CompletionResponse::Array(items)) =
            lspkl_features::completion::completion(ctx.db, ctx.root, position, |r| {
                harness::range(tree, r)
            })
        else {
            panic!("completions")
        };
        assert!(items.iter().any(|i| i.label == "toMap"));
        let hover = lspkl_features::hover::hover(ctx.db, ctx.root, position).unwrap();
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("markdown")
        };
        assert!(
            markup
                .value
                .contains("function toMap(): Map<String, unknown>"),
            "{}",
            markup.value
        );
    });
}
