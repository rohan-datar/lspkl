use super::*;

impl IfExpr {
    pub fn condition(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
}
impl IfExpr {
    pub fn then_branch(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 1)
    }
}
impl IfExpr {
    pub fn else_branch(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 2)
    }
}
impl LetExpr {
    pub fn binding(self, tree: &SyntaxTree) -> Option<NodeId> {
        tree.child(self.0, |kind| {
            matches!(kind, NodeKind::TypedIdentifier | NodeKind::BlankIdentifier)
        })
    }
}
impl LetExpr {
    pub fn value(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
}
impl LetExpr {
    pub fn body(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BinaryExpr(NodeId);
impl BinaryExpr {
    pub fn cast(tree: &SyntaxTree, id: NodeId) -> Option<Self> {
        matches!(
            tree.kind(id),
            NodeKind::AdditiveExpr
                | NodeKind::MultiplicativeExpr
                | NodeKind::ComparisonExpr
                | NodeKind::EqualityExpr
                | NodeKind::ExponentiationExpr
                | NodeKind::LogicalAndExpr
                | NodeKind::LogicalOrExpr
                | NodeKind::NullCoalesceExpr
                | NodeKind::PipeExpr
                | NodeKind::TypeTestExpr
                | NodeKind::TypeCastExpr
        )
        .then_some(Self(id))
    }
    pub fn id(self) -> NodeId {
        self.0
    }
    pub fn left(self, tree: &SyntaxTree) -> Option<NodeId> {
        let operator = tree.child_by_field(self.0, "operator")?;
        tree.children(self.0)
            .iter()
            .copied()
            .take_while(|id| *id != operator)
            .find(|id| Expr::can_cast(tree.kind(*id)) && !tree.is_missing(*id))
    }
    pub fn right(self, tree: &SyntaxTree) -> Option<NodeId> {
        let operator = tree.child_by_field(self.0, "operator")?;
        let category = if matches!(
            tree.kind(self.0),
            NodeKind::TypeTestExpr | NodeKind::TypeCastExpr
        ) {
            TypeNode::can_cast
        } else {
            Expr::can_cast
        };
        tree.children(self.0)
            .iter()
            .copied()
            .skip_while(|id| *id != operator)
            .skip(1)
            .find(|id| category(tree.kind(*id)) && !tree.is_missing(*id))
    }
    pub fn operator(self, tree: &SyntaxTree) -> Option<TokenType> {
        let id = tree.child_by_field(self.0, "operator")?;
        match tree.kind(id) {
            NodeKind::Terminal(token) => Some(*token),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnaryExpr(NodeId);
impl UnaryExpr {
    pub fn cast(tree: &SyntaxTree, id: NodeId) -> Option<Self> {
        matches!(
            tree.kind(id),
            NodeKind::UnaryMinusExpr | NodeKind::LogicalNotExpr | NodeKind::NonNullExpr
        )
        .then_some(Self(id))
    }
    pub fn id(self) -> NodeId {
        self.0
    }
    pub fn operand(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
    pub fn operator(self, tree: &SyntaxTree) -> Option<TokenType> {
        tree.children(self.0)
            .iter()
            .find_map(|id| match tree.kind(*id) {
                NodeKind::Terminal(
                    token @ (TokenType::Minus | TokenType::Not | TokenType::NonNull),
                ) => Some(*token),
                _ => None,
            })
    }
}

impl QualifiedAccessExpr {
    pub fn receiver(self, tree: &SyntaxTree) -> Option<NodeId> {
        tree.child_by_field(self.0, "receiver")
    }
}
impl SubscriptExpr {
    pub fn receiver(self, tree: &SyntaxTree) -> Option<NodeId> {
        tree.child_by_field(self.0, "receiver")
    }
}

impl AmendExpr {
    pub fn parent(self, tree: &SyntaxTree) -> Option<NodeId> {
        tree.child_by_field(self.0, "parent")
    }
}
impl QualifiedAccessExpr {
    pub fn is_null_safe(self, tree: &SyntaxTree) -> bool {
        token(tree, self.0, TokenType::QDot)
    }
}

impl SubscriptExpr {
    pub fn index(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 1)
    }
}

impl NewExpr {
    pub fn ty(self, tree: &SyntaxTree) -> Option<NodeId> {
        ty(tree, self.0, 0)
    }
}

impl FunctionLiteralExpr {
    pub fn parameters(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::ParameterList)
    }
}

impl NewExpr {
    pub fn body(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::ObjectBody)
    }
}
impl AmendExpr {
    pub fn body(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::ObjectBody)
    }
}

impl FunctionLiteralExpr {
    pub fn body(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
}

impl ThrowExpr {
    pub fn value(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
}
impl TraceExpr {
    pub fn value(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
}
impl ReadExpr {
    pub fn value(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
}

impl ParenthesizedExpr {
    pub fn inner(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
}
impl SuperSubscriptExpr {
    pub fn index(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
}
impl UnqualifiedAccessExpr {
    pub fn arguments(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::ArgumentList)
    }
}
impl UnqualifiedAccessExpr {
    pub fn name(self, tree: &SyntaxTree) -> Option<NodeId> {
        tree.identifier(self.0)
    }
}
impl QualifiedAccessExpr {
    pub fn arguments(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::ArgumentList)
    }
}
impl QualifiedAccessExpr {
    pub fn name(self, tree: &SyntaxTree) -> Option<NodeId> {
        tree.identifier(self.0)
    }
}
impl SuperAccessExpr {
    pub fn arguments(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::ArgumentList)
    }
}
impl SuperAccessExpr {
    pub fn name(self, tree: &SyntaxTree) -> Option<NodeId> {
        tree.identifier(self.0)
    }
}

impl SingleLineStringLiteral {
    pub fn interpolations(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0)
            .iter()
            .copied()
            .filter(|id| matches!(tree.kind(*id), NodeKind::StringInterpolation))
    }
}
impl MultiLineStringLiteral {
    pub fn interpolations(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0)
            .iter()
            .copied()
            .filter(|id| matches!(tree.kind(*id), NodeKind::StringInterpolation))
    }
}

impl ImportExpr {
    pub fn uri(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::StringConstant)
    }
}

impl ReadExpr {
    pub fn variant(self, tree: &SyntaxTree) -> Option<TokenType> {
        let id = tree.child_by_field(self.0, "variant")?;
        match tree.kind(id) {
            NodeKind::Terminal(token) => Some(*token),
            _ => None,
        }
    }
}
impl ImportExpr {
    pub fn variant(self, tree: &SyntaxTree) -> Option<TokenType> {
        let id = tree.child_by_field(self.0, "variant")?;
        match tree.kind(id) {
            NodeKind::Terminal(token) => Some(*token),
            _ => None,
        }
    }
}

impl StringInterpolation {
    pub fn value(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
}
