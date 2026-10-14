use lspkl_syntax::ast::{NodeId, NodeKind, SyntaxTree, TokenType, nodes::*};
use lspkl_syntax::parse;

fn find(tree: &SyntaxTree, kind: NodeKind) -> NodeId {
    fn walk(tree: &SyntaxTree, id: NodeId, kind: NodeKind) -> Option<NodeId> {
        if *tree.kind(id) == kind {
            return Some(id);
        }
        tree.children(id)
            .iter()
            .find_map(|id| walk(tree, *id, kind))
    }
    walk(tree, tree.root(), kind).unwrap_or_else(|| panic!("missing {}", kind.name()))
}

fn text(tree: &SyntaxTree, node: Option<NodeId>) -> &str {
    tree.text(node.expect("expected grammar slot"))
}

#[test]
fn interpolation_views_ignore_comments() {
    let tree = parse("x = \"value \\( /* comment */ 1)\"").unwrap();
    let id = find(&tree, NodeKind::SingleLineStringLiteral);
    assert!(
        StringLiteral::cast(&tree, id)
            .unwrap()
            .has_interpolation(&tree)
    );
    let literal = SingleLineStringLiteral::cast(&tree, id).unwrap();
    let interpolation =
        StringInterpolation::cast(&tree, literal.interpolations(&tree).next().unwrap()).unwrap();
    assert_eq!(text(&tree, interpolation.value(&tree)), "1");
}

#[test]
fn complete_grammar_categories_include_formerly_missing_variants() {
    for kind in [NodeKind::ThisExpr, NodeKind::OuterExpr] {
        assert!(Expr::can_cast(&kind));
    }
    assert!(TypeNode::can_cast(&NodeKind::StringLiteralType));
    assert!(ModuleMember::can_cast(&NodeKind::ClassProperty));
    assert!(!Expr::can_cast(&NodeKind::LineComment));
    assert!(!TypeNode::can_cast(&NodeKind::TypeArgumentList));
}

#[test]
fn module_keywords_and_null_safe_access_ignore_comments() {
    for (keyword, amend) in [("amends", true), ("extends", false)] {
        let tree = parse(&format!("/* before */ {keyword} /* target */ \"base.pkl\"\nx = foo /* receiver */ ?. /* member */ bar\n")).unwrap();
        assert_eq!(tree.is_amend(), amend);
        let module = Module::cast(&tree, tree.root()).unwrap();
        let header = ModuleHeader::cast(&tree, module.header(&tree).unwrap()).unwrap();
        let clause =
            ModuleExtendsAmendsClause::cast(&tree, header.extends_amends_clause(&tree).unwrap())
                .unwrap();
        assert_eq!(clause.is_amend(&tree), amend);
        assert_eq!(text(&tree, clause.target(&tree)), "\"base.pkl\"");
        let access =
            QualifiedAccessExpr::cast(&tree, find(&tree, NodeKind::QualifiedAccessExpr)).unwrap();
        assert!(access.is_null_safe(&tree));
        assert_eq!(text(&tree, access.receiver(&tree)), "foo");
        assert_eq!(text(&tree, access.name(&tree)), "bar");
    }
    let tree = parse("x = foo /* ordinary */ . bar").unwrap();
    let access =
        QualifiedAccessExpr::cast(&tree, find(&tree, NodeKind::QualifiedAccessExpr)).unwrap();
    assert!(!access.is_null_safe(&tree));
}

#[test]
fn import_read_and_operator_tokens_are_semantic() {
    for (source, expected, glob) in [
        ("x = import(/* uri */ \"a.pkl\")", TokenType::Import, false),
        (
            "x = import*(/* uri */ \"a.pkl\")",
            TokenType::ImportGlob,
            true,
        ),
    ] {
        let tree = parse(source).unwrap();
        let id = find(&tree, NodeKind::ImportExpr);
        assert_eq!(tree.is_glob_import(id), glob);
        assert_eq!(
            *tree.kind(tree.child_by_field(id, "variant").unwrap()),
            NodeKind::Terminal(expected)
        );
    }
    for (variant, expected) in [
        ("read", TokenType::Read),
        ("read*", TokenType::ReadGlob),
        ("read?", TokenType::ReadOrNull),
    ] {
        let tree = parse(&format!("x = {variant}(/* uri */ \"env:X\")")).unwrap();
        let id = find(&tree, NodeKind::ReadExpr);
        assert_eq!(
            *tree.kind(tree.child_by_field(id, "variant").unwrap()),
            NodeKind::Terminal(expected)
        );
    }
    for (operator, expected, rhs) in [
        ("+", TokenType::Plus, "2"),
        ("-", TokenType::Minus, "2"),
        ("*", TokenType::Star, "2"),
        ("/", TokenType::Slash, "2"),
        ("~/", TokenType::IntDiv, "2"),
        ("%", TokenType::Mod, "2"),
        ("<", TokenType::Less, "2"),
        ("<=", TokenType::LessEqual, "2"),
        (">", TokenType::Greater, "2"),
        (">=", TokenType::GreaterEqual, "2"),
        ("==", TokenType::Equal, "2"),
        ("!=", TokenType::NotEqual, "2"),
        ("&&", TokenType::LogicalAnd, "true"),
        ("||", TokenType::LogicalOr, "true"),
        ("??", TokenType::NullCoalesce, "2"),
        ("**", TokenType::Power, "2"),
        ("|>", TokenType::Pipe, "f"),
        ("as", TokenType::As, "Int"),
        ("is", TokenType::Is, "Int"),
    ] {
        let tree = parse(&format!("x = 1 /* left */ {operator} /* right */ {rhs}")).unwrap();
        let property = ClassProperty::cast(&tree, find(&tree, NodeKind::ClassProperty)).unwrap();
        let binary = BinaryExpr::cast(&tree, property.value(&tree).unwrap()).unwrap();
        assert_eq!(binary.operator(&tree), Some(expected), "{operator}");
        assert_eq!(text(&tree, binary.left(&tree)), "1");
        assert_eq!(text(&tree, binary.right(&tree)), rhs);
    }
}

#[test]
fn conditional_and_let_slots_ignore_comments() {
    let tree = parse("x = if (/* condition */ true) /* then */ 1 else /* else */ \"a\"").unwrap();
    let node = IfExpr::cast(&tree, find(&tree, NodeKind::IfExpr)).unwrap();
    assert_eq!(text(&tree, node.condition(&tree)), "true");
    assert_eq!(text(&tree, node.then_branch(&tree)), "1");
    assert_eq!(text(&tree, node.else_branch(&tree)), "\"a\"");
    let tree = parse("x = let (/* binding */ a: Int = /* value */ 1) /* body */ a").unwrap();
    let node = LetExpr::cast(&tree, find(&tree, NodeKind::LetExpr)).unwrap();
    assert_eq!(text(&tree, node.binding(&tree)), "a: Int");
    assert_eq!(text(&tree, node.value(&tree)), "1");
    assert_eq!(text(&tree, node.body(&tree)), "a");
}

#[test]
fn named_fields_distinguish_member_operands_and_generator_bodies() {
    let tree = parse("x = new { [/* key */ 1] = /* value */ 2; [[/* condition */ true]] = /* value */ 3; when (/* condition */ false) /* then */ { 4 } else /* else */ { 5 }; for (a, b in /* iterable */ items) /* body */ { b } }").unwrap();
    let entry = ObjectEntry::cast(&tree, find(&tree, NodeKind::ObjectEntry)).unwrap();
    assert_eq!(text(&tree, entry.key(&tree)), "1");
    assert_eq!(text(&tree, entry.value(&tree)), "2");
    let predicate = MemberPredicate::cast(&tree, find(&tree, NodeKind::MemberPredicate)).unwrap();
    assert_eq!(text(&tree, predicate.condition(&tree)), "true");
    assert_eq!(text(&tree, predicate.value(&tree)), "3");
    let when = WhenGenerator::cast(&tree, find(&tree, NodeKind::WhenGenerator)).unwrap();
    assert_eq!(text(&tree, when.condition(&tree)), "false");
    assert_eq!(text(&tree, when.then_body(&tree)), "{ 4 }");
    assert_eq!(text(&tree, when.else_body(&tree)), "{ 5 }");
    let generator = ForGenerator::cast(&tree, find(&tree, NodeKind::ForGenerator)).unwrap();
    assert_eq!(text(&tree, generator.iterable(&tree)), "items");
    assert_eq!(text(&tree, generator.body(&tree)), "{ b }");
}

#[test]
fn type_and_list_views_preserve_order_and_empty_function_parameters() {
    let tree = parse("typealias F = (/* first */ Int, /* second */ String) -> /* result */ Boolean\ntypealias G = () -> String").unwrap();
    let module = Module::cast(&tree, tree.root()).unwrap();
    let aliases: Vec<_> = module.members(&tree).collect();
    let first = TypeAlias::cast(&tree, aliases[0]).unwrap();
    let function = FunctionType::cast(&tree, first.ty(&tree).unwrap()).unwrap();
    assert_eq!(
        function
            .parameters(&tree)
            .map(|id| tree.text(id))
            .collect::<Vec<_>>(),
        ["Int", "String"]
    );
    assert_eq!(text(&tree, function.return_type(&tree)), "Boolean");
    let second = TypeAlias::cast(&tree, aliases[1]).unwrap();
    let function = FunctionType::cast(&tree, second.ty(&tree).unwrap()).unwrap();
    assert_eq!(function.parameters(&tree).count(), 0);
    assert_eq!(text(&tree, function.return_type(&tree)), "String");
    let tree = parse("x: /* annotation */ (/* inner */ Int)? = 1\ntypealias Choice = *String | \"a\"\ntypealias Limit = Int(/* constraint */ this > 0)").unwrap();
    let annotation = TypeAnnotation::cast(&tree, find(&tree, NodeKind::TypeAnnotation)).unwrap();
    let nullable = NullableType::cast(&tree, annotation.ty(&tree).unwrap()).unwrap();
    let parenthesized = ParenthesizedType::cast(&tree, nullable.inner(&tree).unwrap()).unwrap();
    assert_eq!(text(&tree, parenthesized.inner(&tree)), "Int");
    let union = UnionType::cast(&tree, find(&tree, NodeKind::UnionType)).unwrap();
    assert_eq!(text(&tree, union.left(&tree)), "*String");
    assert_eq!(text(&tree, union.right(&tree)), "\"a\"");
    let default = DefaultUnionType::cast(&tree, union.left(&tree).unwrap()).unwrap();
    assert_eq!(text(&tree, default.inner(&tree)), "String");
    let constrained = ConstrainedType::cast(&tree, find(&tree, NodeKind::ConstrainedType)).unwrap();
    assert_eq!(text(&tree, constrained.inner(&tree)), "Int");
    assert_eq!(
        constrained
            .constraints(&tree)
            .map(|id| tree.text(id))
            .collect::<Vec<_>>(),
        ["this > 0"]
    );
}

#[test]
fn unary_and_subscript_slots_ignore_comments() {
    let tree = parse("x = items /* receiver */ [/* index */ 1]\ny = - /* operand */ 2").unwrap();
    let subscript = SubscriptExpr::cast(&tree, find(&tree, NodeKind::SubscriptExpr)).unwrap();
    assert_eq!(text(&tree, subscript.receiver(&tree)), "items");
    assert_eq!(text(&tree, subscript.index(&tree)), "1");
    let unary = UnaryExpr::cast(&tree, find(&tree, NodeKind::UnaryMinusExpr)).unwrap();
    assert_eq!(text(&tree, unary.operand(&tree)), "2");
    assert_eq!(unary.operator(&tree), Some(TokenType::Minus));
}

#[test]
fn recovered_binary_operands_keep_their_identity() {
    let tree = parse(include_str!(
        "snapshots/parser/inputs/expr/nestedErrorInBinaryExpr.pkl"
    ))
    .unwrap();
    let binary = BinaryExpr::cast(&tree, find(&tree, NodeKind::LogicalOrExpr)).unwrap();
    assert_eq!(text(&tree, binary.left(&tree)), "one");
    assert_eq!(text(&tree, binary.right(&tree)), "two");
    assert_eq!(binary.operator(&tree), Some(TokenType::LogicalOr));
}

#[test]
fn variance_tokens_survive_comments_and_identifier_prefixes() {
    let tree = parse(
        "class Box<out /* comment */ T, in U, output, input> {}
",
    )
    .unwrap();
    let list = TypeParameterList::cast(&tree, find(&tree, NodeKind::TypeParameterList)).unwrap();
    let parameters: Vec<_> = list.parameters(&tree).collect();
    let variances: Vec<_> = parameters
        .iter()
        .map(|&id| TypeParameter::cast(&tree, id).unwrap().variance(&tree))
        .collect();
    assert_eq!(
        variances,
        [
            Variance::Covariant,
            Variance::Contravariant,
            Variance::Invariant,
            Variance::Invariant
        ]
    );
    assert!(
        tree.children(parameters[0])
            .iter()
            .any(|&id| tree.text(id) == "out")
    );
}
