mod harness;

use lspkl_types::NodeRef;

fn check_call(source: &str, call: &str, expected: &str, argument: Option<&str>) {
    let base = harness::load_test_base();
    harness::with_snippet(&base, source, |ctx| {
        let file = harness::file(ctx, 1);
        let tree = ctx.tree(file).unwrap();
        fn find(
            tree: &lspkl_syntax::ast::SyntaxTree,
            node: lspkl_syntax::ast::NodeId,
            text: &str,
        ) -> Option<lspkl_syntax::ast::NodeId> {
            if tree.text(node) == text {
                return Some(node);
            }
            tree.children(node)
                .iter()
                .find_map(|&n| find(tree, n, text))
        }
        let node = find(tree, tree.root(), call).expect("call");
        let ty = lspkl_types::expr_type(
            ctx.db,
            harness::expr(NodeRef::new(file, node), ctx),
            ctx.root,
        );
        assert_eq!(ty.render(ctx.db), expected, "{call}");
        if let Some(argument) = argument {
            let node = find(tree, node, argument).expect("argument");
            let ty = lspkl_types::expected_type(ctx.db, NodeRef::new(file, node), ctx.root);
            assert_eq!(ty.render(ctx.db), expected, "expected argument type");
        }
    });
}

#[test]
fn explicit_unqualified_call_binds_result_and_parameter() {
    check_call(
        "function identity<T>(x: T): T = x\ny = identity<String>(1)",
        "identity<String>(1)",
        "String",
        Some("1"),
    );
}

#[test]
fn explicit_qualified_call_binds_result_and_parameter() {
    check_call(
        "class Box { function identity<T>(x: T): T = x }\nb = new Box {}\ny = b.identity<String>(1)",
        "b.identity<String>(1)",
        "String",
        Some("1"),
    );
}

#[test]
fn explicit_super_call_binds_result_and_parameter() {
    check_call(
        "open class Parent { function identity<T>(x: T): T = x }\nclass Child extends Parent { y = super.identity<String>(1) }",
        "super.identity<String>(1)",
        "String",
        Some("1"),
    );
}

#[test]
fn stdlib_reference_constructor_keeps_explicit_arguments() {
    check_call(
        "import \"pkl:ref\"\nclass Domain extends ref.AbstractDomain {}\nd = new Domain {}\ny = ref.Reference<Domain, String>(d, \"x\")",
        "ref.Reference<Domain, String>(d, \"x\")",
        "Reference<Domain, String>",
        None,
    );
}

#[test]
fn omitted_generic_arguments_remain_unknown() {
    check_call(
        "function identity<T>(x: T): T = x\ny = identity(1)",
        "identity(1)",
        "unknown",
        None,
    );
}

#[test]
fn method_and_class_type_parameters_are_independent() {
    check_call(
        "class Box<T> { function identity<U>(x: U): Pair<T, U> = Pair<T, U>(null, x) }\nb: Box<Int> = new {}\ny = b.identity<String>(\"a\")",
        "b.identity<String>(\"a\")",
        "Pair<Int, String>",
        None,
    );
}

#[test]
fn explicit_type_arguments_use_the_callers_scope() {
    check_call(
        "class Item {}\nclass Box { function identity<T>(x: T): T = x }\nb = new Box {}\ny = b.identity<Item>(new Item {})",
        "b.identity<Item>(new Item {})",
        "Item",
        Some("new Item {}"),
    );
}

#[test]
fn wrong_type_argument_count_does_not_partially_bind() {
    check_call(
        "function identity<T>(x: T): T = x\ny = identity<String, Int>(1)",
        "identity<String, Int>(1)",
        "unknown",
        Some("1"),
    );
}
