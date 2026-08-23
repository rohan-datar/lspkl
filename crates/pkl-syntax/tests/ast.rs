mod harness;

use pkl_syntax::ast::NodeKind;
use pkl_syntax::parse;

#[test]
fn name_finds_identifier_inside_method_header() {
    let src = "function foo(x: Int): Int = x + 1";
    let tree = parse(src).unwrap();
    let method = tree.children_matching(tree.root(), |k| matches!(k, NodeKind::ClassMethod))[0];
    assert_eq!(tree.name(method), Some("foo"));
}

#[test]
fn doc_comments_returns_stripped_source_slices() {
    let src = "/// Docs for x.\n/// Second line.\nx: Int = 1\ny: Int = 2\n";
    let tree = parse(src).unwrap();
    let prop = tree.children_matching(tree.root(), |k| matches!(k, NodeKind::ClassProperty))[0];
    let docs = tree.doc_comments(prop);
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0], "/// Docs for x.\n/// Second line.");

    // `y` has no doc comment of its own.
    let y = tree.children_matching(tree.root(), |k| matches!(k, NodeKind::ClassProperty))[1];
    assert!(tree.doc_comments(y).is_empty());
}
