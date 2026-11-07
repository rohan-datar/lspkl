mod harness;

use lsp_types::HoverContents;
use lspkl_features::hover::hover;
use lspkl_types::InFile;

#[test]
fn hover_on_generic_call_result_property_has_bound_type() {
    let base = harness::load_test_base();
    let source = "function identity<T>(x: T): T = x\ny = identity<String>(\"a\")\nz = y\n";
    harness::with_snippet(&base, source, |_tree, ctx| {
        let file = harness::file(ctx, 1);
        let offset = harness::offset(ctx.tree(file).unwrap(), 2, 4);
        let result = hover(ctx.db, ctx.root, InFile::new(file, offset)).unwrap();
        let HoverContents::Markup(markup) = result.contents else {
            panic!("markdown hover");
        };
        assert!(markup.value.contains("y: String"), "{}", markup.value);
    });
}

#[test]
fn hover_on_inherited_self_type_result_has_receiver_type() {
    let base = harness::load_test_base();
    let source = "open class Node { children: List<this> }\nclass Special extends Node {}\nn = new Special {}\ny = n.children\nz = y\n";
    harness::with_snippet(&base, source, |_tree, ctx| {
        let file = harness::file(ctx, 1);
        let offset = harness::offset(ctx.tree(file).unwrap(), 4, 4);
        let result = hover(ctx.db, ctx.root, InFile::new(file, offset)).unwrap();
        let HoverContents::Markup(markup) = result.contents else {
            panic!("markdown hover");
        };
        assert!(
            markup.value.contains("y: List<Special>"),
            "{}",
            markup.value
        );
    });
}
