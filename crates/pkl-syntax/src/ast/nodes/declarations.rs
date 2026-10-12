use super::*;

impl Module {
    pub fn members(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        ModuleMember::children(tree, self.0)
    }
}

impl ParameterList {
    pub fn parameters(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0).iter().copied().filter(|id| {
            matches!(
                tree.kind(*id),
                NodeKind::TypedIdentifier | NodeKind::BlankIdentifier
            )
        })
    }
}
impl TypeParameterList {
    pub fn parameters(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0)
            .iter()
            .copied()
            .filter(|id| matches!(tree.kind(*id), NodeKind::TypeParameter))
    }
}

impl ClassBody {
    pub fn members(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0).iter().copied().filter(|id| {
            matches!(
                tree.kind(*id),
                NodeKind::ClassProperty | NodeKind::ClassMethod
            )
        })
    }
}
impl Module {
    pub fn imports(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0)
            .iter()
            .copied()
            .filter(|id| matches!(tree.kind(*id), NodeKind::Import | NodeKind::ImportGlob))
    }
}
impl Class {
    pub fn type_parameters(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::TypeParameterList)
    }
}
impl TypeAlias {
    pub fn type_parameters(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::TypeParameterList)
    }
}
impl MethodHeader {
    pub fn type_parameters(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::TypeParameterList)
    }
}
impl Module {
    pub fn header(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::ModuleHeader)
    }
}
impl ModuleHeader {
    pub fn extends_amends_clause(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::ModuleExtendsAmendsClause)
    }
}
impl Class {
    pub fn body(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::ClassBody)
    }
}
impl MethodHeader {
    pub fn parameters(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::ParameterList)
    }
}
impl MethodHeader {
    pub fn type_annotation(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::TypeAnnotation)
    }
}
impl TypedIdentifier {
    pub fn type_annotation(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::TypeAnnotation)
    }
}

impl Annotation {
    pub fn name(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::QualifiedIdentifier)
    }
}
impl Annotation {
    pub fn body(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::ObjectBody)
    }
}

impl ClassExtendsClause {
    pub fn name(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::QualifiedIdentifier)
    }
}
impl ClassExtendsClause {
    pub fn arguments(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::TypeArgumentList)
    }
}

impl ModuleExtendsAmendsClause {
    pub fn is_amend(self, tree: &SyntaxTree) -> bool {
        token(tree, self.0, TokenType::Amends)
    }
}
impl ModuleExtendsAmendsClause {
    pub fn target(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::StringConstant)
    }
}
impl Class {
    pub fn extends_target(self, tree: &SyntaxTree) -> Option<NodeId> {
        let clause = of_kind(tree, self.0, NodeKind::ClassExtendsClause)?;
        of_kind(tree, clause, NodeKind::QualifiedIdentifier)
    }
}

impl ModuleHeader {
    pub fn annotations(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0)
            .iter()
            .copied()
            .filter(|id| matches!(tree.kind(*id), NodeKind::Annotation))
    }
}
impl Class {
    pub fn annotations(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0)
            .iter()
            .copied()
            .filter(|id| matches!(tree.kind(*id), NodeKind::Annotation))
    }
}
impl TypeAlias {
    pub fn annotations(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0)
            .iter()
            .copied()
            .filter(|id| matches!(tree.kind(*id), NodeKind::Annotation))
    }
}
impl ClassProperty {
    pub fn annotations(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0)
            .iter()
            .copied()
            .filter(|id| matches!(tree.kind(*id), NodeKind::Annotation))
    }
}
impl ClassMethod {
    pub fn annotations(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0)
            .iter()
            .copied()
            .filter(|id| matches!(tree.kind(*id), NodeKind::Annotation))
    }
}

impl QualifiedIdentifier {
    pub fn identifiers(self, tree: &SyntaxTree) -> impl Iterator<Item = NodeId> + '_ {
        tree.children(self.0)
            .iter()
            .copied()
            .filter(|id| matches!(tree.kind(*id), NodeKind::Terminal(TokenType::Identifier)))
    }
}
impl Import {
    pub fn uri(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::StringConstant)
    }
}
impl ImportGlob {
    pub fn uri(self, tree: &SyntaxTree) -> Option<NodeId> {
        of_kind(tree, self.0, NodeKind::StringConstant)
    }
}
