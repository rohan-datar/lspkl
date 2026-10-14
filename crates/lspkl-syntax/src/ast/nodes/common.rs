use super::*;

pub(super) fn token(tree: &SyntaxTree, id: NodeId, expected: TokenType) -> bool {
    tree.child(id, |kind| *kind == NodeKind::Terminal(expected))
        .is_some()
}
pub(super) fn positional(
    tree: &SyntaxTree,
    id: NodeId,
    index: usize,
    category: fn(&NodeKind) -> bool,
) -> Option<NodeId> {
    // Comments and punctuation do not occupy a grammar operand slot. Recovery nodes do.
    let child = tree
        .children(id)
        .iter()
        .copied()
        .filter(|child| category(tree.kind(*child)) || matches!(tree.kind(*child), NodeKind::Error))
        .nth(index)?;
    (category(tree.kind(child)) && !tree.is_missing(child)).then_some(child)
}
pub(super) fn expression(tree: &SyntaxTree, id: NodeId, index: usize) -> Option<NodeId> {
    positional(tree, id, index, Expr::can_cast)
}
pub(super) fn ty(tree: &SyntaxTree, id: NodeId, index: usize) -> Option<NodeId> {
    positional(tree, id, index, TypeNode::can_cast)
}
pub(super) fn of_kind(tree: &SyntaxTree, id: NodeId, kind: NodeKind) -> Option<NodeId> {
    tree.child(id, |k| *k == kind)
}

impl ArgumentList {
    pub fn arguments(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        Expr::children(tree, self.0)
    }
}

category!(StringLiteral: SingleLineStringLiteral, MultiLineStringLiteral, StringConstant, StringLiteralType);
impl StringLiteral {
    pub fn has_interpolation(self, tree: &SyntaxTree) -> bool {
        tree.child(self.id(), |kind| {
            matches!(kind, NodeKind::StringInterpolation)
        })
        .is_some()
    }
}
