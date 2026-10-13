mod harness;

use lsp_features::{goto_definition::goto_definition, hover::hover};
use lsp_types::{GotoDefinitionResponse, HoverContents, Position, Range};
use pkl_types::InFile;

#[test]
fn constrained_receiver_keeps_qualified_hover_and_definition() {
    let base = harness::load_test_base();
    let source = "class Box {\n  /// The stored text.\n  value: String\n}\nbox: Box(true)\nresult = box.value\n";
    harness::with_snippet(&base, source, |_tree, ctx| {
        let cursor = InFile::new(
            harness::file(ctx, 1),
            source.rfind("value").unwrap().try_into().unwrap(),
        );
        let hovered = hover(ctx, cursor).unwrap();
        let HoverContents::Markup(markup) = hovered.contents else {
            panic!("markdown hover")
        };
        assert!(markup.value.contains("value: String"), "{}", markup.value);
        assert!(
            markup.value.contains("The stored text."),
            "{}",
            markup.value
        );
        let uri: lsp_types::Uri = "file:///snippets/refined.pkl".parse().unwrap();
        let definition = goto_definition(
            ctx,
            cursor,
            |file| (file == harness::file(ctx, 1)).then(|| uri.clone()),
            harness::range,
        )
        .unwrap();
        let GotoDefinitionResponse::Scalar(location) = definition else {
            panic!("single declaration")
        };
        assert_eq!(location.uri, uri);
        assert_eq!(
            location.range,
            Range::new(Position::new(1, 2), Position::new(2, 15))
        );
    });
}
