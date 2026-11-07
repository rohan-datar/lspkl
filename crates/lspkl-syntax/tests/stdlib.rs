use lspkl_syntax::{
    ast::{NodeId, NodeKind, SyntaxTree},
    parse,
};

fn assert_valid(tree: &SyntaxTree, id: NodeId, module: &str) {
    assert_ne!(
        *tree.kind(id),
        NodeKind::Error,
        "pkl:{module}: error at {:?}: {}",
        tree.span(id),
        tree.text(id)
    );
    assert!(
        !tree.is_missing(id),
        "pkl:{module}: missing {} at {:?}",
        tree.kind(id).name(),
        tree.span(id)
    );
    for &child in tree.children(id) {
        assert_valid(tree, child, module);
    }
}

#[test]
fn every_embedded_sdk_module_parses_without_errors_or_missing_nodes() {
    assert!(lspkl_stdlib::MODULE_NAMES.contains(&"net"));
    assert!(lspkl_stdlib::MODULE_NAMES.contains(&"syntax"));
    for &module in lspkl_stdlib::MODULE_NAMES {
        let source = lspkl_stdlib::source(module).expect("listed SDK module must be embedded");
        let tree = parse(source).expect("SDK module must parse");
        assert_valid(&tree, tree.root(), module);
    }
}
