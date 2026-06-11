mod harness;

use pkl_analysis::ast::NodeKind;
use pkl_analysis::parse;

#[test]
fn name_finds_identifier_inside_method_header() {
    let src = "function foo(x: Int): Int = x + 1";
    let tree = parse(src).unwrap();
    let method = tree.children_matching(tree.root(), |k| matches!(k, NodeKind::ClassMethod))[0];
    assert_eq!(tree.name(method), Some("foo"));
}
