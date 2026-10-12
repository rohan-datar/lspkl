use super::*;

impl ObjectEntry {
    pub fn key(self, tree: &SyntaxTree) -> Option<NodeId> {
        tree.child_by_field(self.0, "key")
    }
}
impl ObjectEntry {
    pub fn value(self, tree: &SyntaxTree) -> Option<NodeId> {
        tree.child_by_field(self.0, "valueExpr")
    }
}
impl MemberPredicate {
    pub fn condition(self, tree: &SyntaxTree) -> Option<NodeId> {
        tree.child_by_field(self.0, "conditionExpr")
    }
}
impl MemberPredicate {
    pub fn value(self, tree: &SyntaxTree) -> Option<NodeId> {
        tree.child_by_field(self.0, "valueExpr")
    }
}
impl WhenGenerator {
    pub fn condition(self, tree: &SyntaxTree) -> Option<NodeId> {
        tree.child_by_field(self.0, "conditionExpr")
    }
}
impl WhenGenerator {
    pub fn then_body(self, tree: &SyntaxTree) -> Option<NodeId> {
        tree.child_by_field(self.0, "thenBody")
    }
}
impl WhenGenerator {
    pub fn else_body(self, tree: &SyntaxTree) -> Option<NodeId> {
        tree.child_by_field(self.0, "elseBody")
    }
}

impl ForGenerator {
    pub fn iterable(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
}
impl ForGenerator {
    pub fn body(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::ObjectBody)
    }
}

impl ObjectBody {
    pub fn members(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        ObjectMember::children(tree, self.0)
    }
}

impl ClassMethod {
    pub fn header(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::MethodHeader)
    }
}
impl ClassMethod {
    pub fn body(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
}
impl ObjectMethod {
    pub fn header(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::MethodHeader)
    }
}
impl ObjectMethod {
    pub fn body(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
}
impl ClassProperty {
    pub fn type_annotation(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::TypeAnnotation)
    }
}
impl ClassProperty {
    pub fn value(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
}
impl ClassProperty {
    pub fn body(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::ObjectBody)
    }
}
impl ClassProperty {
    pub fn bodies(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0)
            .iter()
            .copied()
            .filter(|id| matches!(tree.kind(*id), NodeKind::ObjectBody))
    }
}
impl ObjectProperty {
    pub fn type_annotation(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::TypeAnnotation)
    }
}
impl ObjectProperty {
    pub fn value(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
}
impl ObjectProperty {
    pub fn body(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::ObjectBody)
    }
}
impl ObjectProperty {
    pub fn bodies(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0)
            .iter()
            .copied()
            .filter(|id| matches!(tree.kind(*id), NodeKind::ObjectBody))
    }
}
impl ObjectEntry {
    pub fn body(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::ObjectBody)
    }
}
impl ObjectEntry {
    pub fn bodies(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0)
            .iter()
            .copied()
            .filter(|id| matches!(tree.kind(*id), NodeKind::ObjectBody))
    }
}
impl MemberPredicate {
    pub fn body(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::ObjectBody)
    }
}
impl MemberPredicate {
    pub fn bodies(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0)
            .iter()
            .copied()
            .filter(|id| matches!(tree.kind(*id), NodeKind::ObjectBody))
    }
}

impl ObjectElement {
    pub fn value(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
}
impl ObjectSpread {
    pub fn value(self, tree: &SyntaxTree) -> Option<NodeId> {
        expression(tree, self.0, 0)
    }
}
