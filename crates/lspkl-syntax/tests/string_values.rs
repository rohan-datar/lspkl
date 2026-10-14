use lspkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};
fn string_node(tree: &SyntaxTree, node: NodeId) -> Option<NodeId> {
    if tree.kind(node).is_string_literal() {
        return Some(node);
    }
    tree.children(node)
        .iter()
        .find_map(|&n| string_node(tree, n))
}
#[test]
fn import_literals_decode_normal_unicode_and_raw_escapes() {
    for (literal, expected) in [
        (r#""caf\u{e9}.pkl""#, "café.pkl"),
        (r#""dir\\file\"name.pkl""#, "dir\\file\"name.pkl"),
        (r##"#"literal\path.pkl"#"##, "literal\\path.pkl"),
        (r##"#"caf\#u{e9}.pkl"#"##, "café.pkl"),
        (r#""日本.pkl""#, "日本.pkl"),
    ] {
        let tree = lspkl_syntax::parse(&format!("import {literal}\n")).unwrap();
        let node = string_node(&tree, tree.root()).unwrap();
        assert!(matches!(tree.kind(node), NodeKind::StringConstant));
        assert_eq!(
            tree.escaped_string_literal(node).as_deref(),
            Some(expected),
            "{literal}"
        );
    }
}
#[test]
fn interpolated_and_invalid_literals_have_no_constant_value() {
    for literal in [r#""\(name).pkl""#, r#""\u{110000}.pkl""#, r#""\q.pkl""#] {
        let tree = lspkl_syntax::parse(&format!("value = {literal}\n")).unwrap();
        assert!(
            string_node(&tree, tree.root())
                .and_then(|node| tree.escaped_string_literal(node))
                .is_none(),
            "{literal}"
        );
    }
}

#[test]
fn expression_literals_support_multiple_raw_delimiters() {
    let tree = lspkl_syntax::parse(r###"value = ##"caf\##u{e9}.pkl"##"###).unwrap();
    let node = string_node(&tree, tree.root()).unwrap();
    assert_eq!(
        tree.escaped_string_literal(node).as_deref(),
        Some("café.pkl")
    );
}
