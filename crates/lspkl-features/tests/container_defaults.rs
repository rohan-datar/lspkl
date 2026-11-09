mod harness;

use lsp_types::{CompletionResponse, HoverContents};
use lspkl_syntax::TextSize;
use lspkl_types::InFile;

#[test]
fn parameterized_default_body_completion_details_use_shared_member_types() {
    let base = harness::load_test_base();
    for (annotation, expected) in [
        ("Mapping<String, List<Int>>", "Function1<String, List<Int>>"),
        ("Listing<String>", "Function1<Int, String>"),
    ] {
        let source = format!("value: {annotation} = new {{\n  \n}}\n");
        harness::with_snippet(&base, &source, |tree, ctx| {
            let offset = TextSize::try_from(source.find("  ").unwrap() + 1).unwrap();
            let response = lspkl_features::completion::completion(
                ctx.db,
                ctx.root,
                InFile::new(harness::file(ctx, 1), offset),
                |r| harness::range(tree, r),
            );
            let Some(CompletionResponse::Array(items)) = response else {
                panic!("completions")
            };
            for label in ["default = ", "default {…}"] {
                let item = items
                    .iter()
                    .find(|i| i.label == label)
                    .unwrap_or_else(|| panic!("{items:?}"));
                assert_eq!(item.detail.as_deref(), Some(expected));
            }
        });
    }
}

#[test]
fn parameterized_default_access_hover_preserves_bindings() {
    let base = harness::load_test_base();
    for (annotation, expected) in [
        ("Mapping<String, List<Int>>", "Function1<String, List<Int>>"),
        ("Listing<String>", "Function1<Int, String>"),
    ] {
        let source = format!("value: {annotation} = new {{}}\nresult = value.default\n");
        harness::with_snippet(&base, &source, |_tree, ctx| {
            let offset = TextSize::try_from(source.find("default").unwrap() + 2).unwrap();
            let hover = lspkl_features::hover::hover(
                ctx.db,
                ctx.root,
                InFile::new(harness::file(ctx, 1), offset),
            )
            .unwrap();
            let HoverContents::Markup(markup) = hover.contents else {
                panic!("markdown")
            };
            assert!(markup.value.contains(expected), "{}", markup.value);
        });
    }
}
