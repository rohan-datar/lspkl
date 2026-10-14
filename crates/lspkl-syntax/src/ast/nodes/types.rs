use super::*;

impl TypeAnnotation {
    pub fn ty(self, tree: &SyntaxTree) -> Option<NodeId> {
        ty(tree, self.0, 0)
    }
}
impl TypeAlias {
    pub fn ty(self, tree: &SyntaxTree) -> Option<NodeId> {
        ty(tree, self.0, 0)
    }
}

impl ParenthesizedType {
    pub fn inner(self, tree: &SyntaxTree) -> Option<NodeId> {
        ty(tree, self.0, 0)
    }
}
impl NullableType {
    pub fn inner(self, tree: &SyntaxTree) -> Option<NodeId> {
        ty(tree, self.0, 0)
    }
}
impl DefaultUnionType {
    pub fn inner(self, tree: &SyntaxTree) -> Option<NodeId> {
        ty(tree, self.0, 0)
    }
}
impl ConstrainedType {
    pub fn inner(self, tree: &SyntaxTree) -> Option<NodeId> {
        ty(tree, self.0, 0)
    }
}
impl UnionType {
    pub fn left(self, tree: &SyntaxTree) -> Option<NodeId> {
        ty(tree, self.0, 0)
    }
}
impl UnionType {
    pub fn right(self, tree: &SyntaxTree) -> Option<NodeId> {
        ty(tree, self.0, 1)
    }
}
impl FunctionType {
    pub fn return_type(self, tree: &SyntaxTree) -> Option<NodeId> {
        let arrow = tree
            .children(self.0)
            .iter()
            .position(|id| *tree.kind(*id) == NodeKind::Terminal(TokenType::Arrow))?;
        tree.children(self.0)[arrow + 1..]
            .iter()
            .copied()
            .find(|id| TypeNode::can_cast(tree.kind(*id)))
    }
}
impl FunctionType {
    pub fn parameters(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0)
            .iter()
            .copied()
            .take_while(|id| *tree.kind(*id) != NodeKind::Terminal(TokenType::Arrow))
            .filter(|id| TypeNode::can_cast(tree.kind(*id)))
    }
}
impl ConstrainedType {
    pub fn constraints(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        Expr::children(tree, self.0)
    }
}

impl TypeArgumentList {
    pub fn arguments(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        TypeNode::children(tree, self.0)
    }
}

impl DeclaredType {
    pub fn arguments(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0)
            .iter()
            .copied()
            .filter(|id| matches!(tree.kind(*id), NodeKind::TypeArgumentList))
            .flat_map(|id| TypeNode::children(tree, id))
    }
}

impl DeclaredType {
    pub fn name(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::QualifiedIdentifier)
    }
}
