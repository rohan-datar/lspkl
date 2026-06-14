//! Index-based syntax tree.
//!
//!  Every node lives in a `Vec` inside [`SyntaxTree`] and is
//! referenced by [`NodeId`].

use smallvec::SmallVec;

/// Opaque handle into a [`SyntaxTree`]'s node arena.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(pub(crate) u32);

impl NodeId {
    pub const ROOT: NodeId = NodeId(0);

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// 1-indexed line/column source span.
///
/// `byte_range` is preserved alongside so we can extract the underlying source text
/// without reconstructing offsets from line/column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub begin_line: u32,
    pub begin_col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub begin_byte: u32,
    pub end_byte: u32,
}

impl Span {
    /// Does `(line, col)` fall inside this span (inclusive)?
    pub fn matches(&self, line: u32, col: u32) -> bool {
        if line < self.begin_line || line > self.end_line {
            return false;
        }
        if self.begin_line == self.end_line {
            col >= self.begin_col && col <= self.end_col
        } else if line == self.begin_line {
            col >= self.begin_col
        } else if line == self.end_line {
            col <= self.end_col
        } else {
            true
        }
    }

    pub fn byte_range(&self) -> std::ops::Range<usize> {
        self.begin_byte as usize..self.end_byte as usize
    }
}

/// All node kinds in the Pkl AST, mapped from tree-sitter's grammar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    // ---- Top-level ----
    Module,
    ModuleHeader,
    ModuleClause,
    ModuleExtendsAmendsClause,
    Import,
    ImportGlob,

    // ---- Declarations ----
    Class,
    ClassBody,
    ClassExtendsClause,
    TypeAlias,
    Annotation,

    // ---- Members ----
    ClassProperty,
    ClassMethod,
    ObjectProperty,
    ObjectMethod,
    MethodHeader,
    ParameterList,
    TypeParameterList,
    TypeParameter,
    TypedIdentifier,
    BlankIdentifier,
    TypeAnnotation,
    ArgumentList,

    // ---- Object body ----
    ObjectBody,
    ObjectEntry,
    ObjectElement,
    ObjectSpread,
    MemberPredicate,
    ForGenerator,
    WhenGenerator,

    // ---- Expressions ----
    ThisExpr,
    OuterExpr,
    ModuleExpr,
    NullLiteralExpr,
    TrueLiteralExpr,
    FalseLiteralExpr,
    IntLiteralExpr,
    FloatLiteralExpr,
    ThrowExpr,
    TraceExpr,
    ImportExpr,
    ReadExpr,
    SingleLineStringLiteral,
    MultiLineStringLiteral,
    NewExpr,
    AmendExpr,
    UnqualifiedAccessExpr,
    QualifiedAccessExpr,
    SuperAccessExpr,
    SuperSubscriptExpr,
    SubscriptExpr,
    NonNullExpr,
    UnaryMinusExpr,
    LogicalNotExpr,
    AdditiveExpr,
    MultiplicativeExpr,
    ComparisonExpr,
    EqualityExpr,
    ExponentiationExpr,
    LogicalAndExpr,
    LogicalOrExpr,
    NullCoalesceExpr,
    PipeExpr,
    TypeTestExpr,
    TypeCastExpr,
    IfExpr,
    LetExpr,
    FunctionLiteralExpr,
    ParenthesizedExpr,

    // ---- Type annotations (in source) ----
    UnknownType,
    NothingType,
    ModuleType,
    StringLiteralType,
    DeclaredType,
    ParenthesizedType,
    NullableType,
    ConstrainedType,
    UnionType,
    FunctionType,
    DefaultUnionType,
    TypeArgumentList,

    // ---- Other ----
    QualifiedIdentifier,
    StringConstant,
    StringInterpolation,
    Terminal(TokenType),
    Error,
    LineComment,
    BlockComment,
    ShebangComment,

    /// A tree-sitter node type we have not mapped to a concrete variant yet.
    Unmapped(String),
}

impl NodeKind {
    pub fn name(&self) -> &str {
        match self {
            NodeKind::Terminal(_) => "Terminal",
            NodeKind::Unmapped(raw) => raw.as_str(),
            other => kind_static_name(other),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, NodeKind::Terminal(_))
    }

    pub fn is_module_member(&self) -> bool {
        matches!(
            self,
            NodeKind::Class | NodeKind::TypeAlias | NodeKind::ClassMethod
        )
    }

    pub fn is_string_literal(&self) -> bool {
        matches!(
            self,
            NodeKind::SingleLineStringLiteral
                | NodeKind::MultiLineStringLiteral
                | NodeKind::StringConstant
                | NodeKind::StringLiteralType
        )
    }

    pub fn is_object_member(&self) -> bool {
        matches!(
            self,
            NodeKind::ObjectProperty
                | NodeKind::ObjectEntry
                | NodeKind::MemberPredicate
                | NodeKind::ForGenerator
                | NodeKind::WhenGenerator
                | NodeKind::ObjectElement
                | NodeKind::ObjectSpread
                | NodeKind::ObjectMethod
        )
    }

    pub fn is_container(&self) -> bool {
        matches!(
            self,
            NodeKind::Class
                | NodeKind::ClassMethod
                | NodeKind::ClassProperty
                | NodeKind::ObjectProperty
                | NodeKind::ObjectEntry
                | NodeKind::ObjectElement
                | NodeKind::ObjectMethod
                | NodeKind::MemberPredicate
                | NodeKind::FunctionLiteralExpr
        )
    }

    pub fn is_type_node(&self) -> bool {
        matches!(
            self,
            NodeKind::UnknownType
                | NodeKind::NothingType
                | NodeKind::ModuleType
                | NodeKind::DeclaredType
                | NodeKind::ParenthesizedType
                | NodeKind::NullableType
                | NodeKind::ConstrainedType
                | NodeKind::UnionType
                | NodeKind::FunctionType
                | NodeKind::DefaultUnionType
        )
    }

    pub fn is_expr_node(&self) -> bool {
        matches!(
            self,
            NodeKind::ModuleExpr
                | NodeKind::NullLiteralExpr
                | NodeKind::TrueLiteralExpr
                | NodeKind::FalseLiteralExpr
                | NodeKind::IntLiteralExpr
                | NodeKind::FloatLiteralExpr
                | NodeKind::ThrowExpr
                | NodeKind::TraceExpr
                | NodeKind::ImportExpr
                | NodeKind::ReadExpr
                | NodeKind::SingleLineStringLiteral
                | NodeKind::MultiLineStringLiteral
                | NodeKind::NewExpr
                | NodeKind::AmendExpr
                | NodeKind::UnqualifiedAccessExpr
                | NodeKind::QualifiedAccessExpr
                | NodeKind::SuperAccessExpr
                | NodeKind::SuperSubscriptExpr
                | NodeKind::SubscriptExpr
                | NodeKind::NonNullExpr
                | NodeKind::UnaryMinusExpr
                | NodeKind::LogicalNotExpr
                | NodeKind::AdditiveExpr
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
                | NodeKind::IfExpr
                | NodeKind::LetExpr
                | NodeKind::FunctionLiteralExpr
                | NodeKind::ParenthesizedExpr
        )
    }

    pub fn is_module_uri_owner(&self) -> bool {
        matches!(
            self,
            NodeKind::Import
                | NodeKind::ImportGlob
                | NodeKind::ImportExpr
                | NodeKind::ModuleExtendsAmendsClause
        )
    }
}

/// Token classes for terminal nodes (modifiers, keywords, operators, identifiers).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenType {
    Identifier,
    DocComment,
    StringLiteral,

    // Modifiers
    Abstract,
    External,
    Hidden,
    Local,
    Open,
    Fixed,
    Const,

    // Operators / punctuation - extend as the lexer surfaces them
    Plus,
    Minus,
    Star,
    Slash,
    IntDiv,
    Mod,
    Is,
    As,
    NonNull,
    QDot,

    // Keywords driving import/read forms
    Import,
    ImportGlob,
    ReadOrNull,
    ReadGlob,

    /// Anything not classified yet. Carries no data; the actual token text
    /// can be retrieved via `SyntaxTree::text(node)`.
    Other,
}

impl TokenType {
    pub fn is_modifier(self) -> bool {
        matches!(
            self,
            TokenType::Abstract
                | TokenType::External
                | TokenType::Hidden
                | TokenType::Local
                | TokenType::Open
                | TokenType::Fixed
                | TokenType::Const
        )
    }
}

/// Internal storage for a single node.
#[derive(Debug)]
pub(crate) struct NodeEntry {
    pub kind: NodeKind,
    pub span: Span,
    pub parent: Option<NodeId>,
    pub children: SmallVec<[NodeId; 6]>,
    pub is_missing: bool,
}

/// The complete syntax tree for one parsed file.
#[derive(Debug)]
pub struct SyntaxTree {
    nodes: Vec<NodeEntry>,
    source: String,
}

impl SyntaxTree {
    pub(crate) fn from_parts(source: String, nodes: Vec<NodeEntry>) -> Self {
        debug_assert!(!nodes.is_empty(), "tree must contain at least a root node");
        Self { nodes, source }
    }

    pub fn root(&self) -> NodeId {
        NodeId::ROOT
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn kind(&self, id: NodeId) -> &NodeKind {
        &self.nodes[id.index()].kind
    }

    pub fn span(&self, id: NodeId) -> Span {
        self.nodes[id.index()].span
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.index()].parent
    }

    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.nodes[id.index()].children
    }

    pub fn is_missing(&self, id: NodeId) -> bool {
        self.nodes[id.index()].is_missing
    }

    pub fn text(&self, id: NodeId) -> &str {
        let span = self.span(id);
        &self.source[span.byte_range()]
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Walk ancestors until one matches `predicate`. Skips the node itself.
    pub fn ancestor<F: Fn(&NodeKind) -> bool>(&self, id: NodeId, predicate: F) -> Option<NodeId> {
        let mut current = self.parent(id);
        while let Some(nid) = current {
            if predicate(self.kind(nid)) {
                return Some(nid);
            }
            current = self.parent(nid);
        }
        None
    }

    /// Find the deepest node whose span contains `(line, col)`.
    /// Lines and columns are 1-indexed.
    pub fn find_at(&self, line: u32, col: u32) -> Option<NodeId> {
        self.find_at_recursive(NodeId::ROOT, line, col)
    }

    fn find_at_recursive(&self, id: NodeId, line: u32, col: u32) -> Option<NodeId> {
        for &child in self.children(id) {
            if self.span(child).matches(line, col) {
                return self.find_at_recursive(child, line, col).or(Some(child));
            }
        }
        if self.span(id).matches(line, col) {
            Some(id)
        } else {
            None
        }
    }

    /// First child whose kind matches the predicate.
    pub fn child<F: Fn(&NodeKind) -> bool>(&self, id: NodeId, predicate: F) -> Option<NodeId> {
        self.children(id)
            .iter()
            .copied()
            .find(|&c| predicate(self.kind(c)))
    }

    /// All children whose kind matches the predicate.
    pub fn children_matching<F: Fn(&NodeKind) -> bool>(
        &self,
        id: NodeId,
        predicate: F,
    ) -> Vec<NodeId> {
        self.children(id)
            .iter()
            .copied()
            .filter(|&c| predicate(self.kind(c)))
            .collect()
    }

    /// Identifier terminal child (for class/property/method name lookups).
    pub fn identifier(&self, id: NodeId) -> Option<NodeId> {
        let holder = match self.kind(id) {
            NodeKind::ClassMethod | NodeKind::ObjectMethod => self
                .child(id, |k| matches!(k, NodeKind::MethodHeader))
                .unwrap_or(id),
            _ => id,
        };
        self.child(holder, |k| {
            matches!(k, NodeKind::Terminal(TokenType::Identifier))
        })
    }

    pub fn modifiers(&self, id: NodeId) -> Vec<TokenType> {
        let holder = match self.kind(id) {
            NodeKind::ClassMethod | NodeKind::ObjectMethod => self
                .child(id, |k| matches!(k, NodeKind::MethodHeader))
                .unwrap_or(id),
            _ => id,
        };
        self.children(holder)
            .iter()
            .copied()
            .filter_map(|c| match self.kind(c) {
                NodeKind::Terminal(tt) if tt.is_modifier() => Some(*tt),
                _ => None,
            })
            .collect()
    }

    pub fn name(&self, id: NodeId) -> Option<&str> {
        self.identifier(id)
            .map(|nid| unquote_identifier(self.text(nid)))
    }

    // find the module name and simple type name of a declared type
    pub fn declared_type_parts(&self, id: NodeId) -> Vec<&str> {
        if !matches!(self.kind(id), NodeKind::DeclaredType) {
            return vec![];
        }
        let Some(qid) = self.child(id, |c| matches!(c, NodeKind::QualifiedIdentifier)) else {
            return vec![];
        };
        self.qualified_identifier_segments(qid)
    }

    pub fn qualified_identifier_segments(&self, id: NodeId) -> Vec<&str> {
        if !matches!(self.kind(id), NodeKind::QualifiedIdentifier) {
            return vec![];
        }
        self.children_matching(id, |c| {
            matches!(c, NodeKind::Terminal(TokenType::Identifier))
        })
        .iter()
        .map(|&n| unquote_identifier(self.text(n)))
        .collect()
    }

    pub fn has_modifier(&self, id: NodeId, modifier: TokenType) -> bool {
        self.modifiers(id).contains(&modifier)
    }

    pub fn is_amend(&self) -> bool {
        let Some(header) = self.child(self.root(), |c| matches!(c, NodeKind::ModuleHeader)) else {
            return false;
        };
        let Some(header_clause) =
            self.child(header, |c| matches!(c, NodeKind::ModuleExtendsAmendsClause))
        else {
            return false;
        };

        self.text(header_clause).starts_with("amends")
    }

    /// For a `Class` node, return the node representing its `extends` target
    /// (the type after the `extends` keyword), if any.
    pub fn class_extends_target(&self, class_node: NodeId) -> Option<NodeId> {
        if !matches!(self.kind(class_node), NodeKind::Class) {
            return None;
        }

        let extends_clause =
            self.child(class_node, |n| matches!(n, NodeKind::ClassExtendsClause))?;

        self.child(extends_clause, |n| !matches!(n, NodeKind::Terminal(_)))
    }

    pub fn module_extends_or_amends_target(&self) -> Option<String> {
        let header = self.child(self.root(), |k| matches!(k, NodeKind::ModuleHeader))?;
        let extend_amend_clause =
            self.child(header, |k| matches!(k, NodeKind::ModuleExtendsAmendsClause))?;
        let target = self.child(extend_amend_clause, |k| k.is_string_literal())?;
        self.escaped_string_literal(target)
    }

    pub fn type_alias_body(&self, alias_node: NodeId) -> Option<NodeId> {
        self.child(alias_node, |c| c.is_type_node())
    }

    pub fn escaped_string_literal(&self, string_node: NodeId) -> Option<String> {
        if !self.kind(string_node).is_string_literal() {
            return None;
        }

        let lit = self.text(string_node);

        Some(
            lit.trim_matches('"')
                .chars()
                .flat_map(char::escape_default)
                .collect(),
        )
    }

    pub fn is_glob_import(&self, node: NodeId) -> bool {
        match self.kind(node) {
            NodeKind::ImportGlob => true,
            NodeKind::ImportExpr => self.text(node).starts_with("import*"),
            _ => false,
        }
    }

    pub fn is_type_name(&self, node: NodeId) -> bool {
        if !matches!(self.kind(node), NodeKind::QualifiedIdentifier) {
            return false;
        }
        let Some(parent) = self.parent(node) else {
            return false;
        };
        matches!(
            self.kind(parent),
            NodeKind::DeclaredType | NodeKind::ClassExtendsClause
        )
    }

    pub fn is_property_access(&self, node: NodeId) -> bool {
        if !matches!(self.kind(node), NodeKind::UnqualifiedAccessExpr) {
            return false;
        }
        self.child(node, |k| matches!(k, NodeKind::ArgumentList))
            .is_none()
    }
}

/// Long-name list used by `NodeKind::name`.
fn kind_static_name(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Module => "Module",
        NodeKind::ModuleHeader => "ModuleHeader",
        NodeKind::ModuleClause => "ModuleClause",
        NodeKind::ModuleExtendsAmendsClause => "ModuleExtendsAmendsClause",
        NodeKind::Import => "Import",
        NodeKind::ImportGlob => "ImportGlob",
        NodeKind::Class => "Class",
        NodeKind::ClassBody => "ClassBody",
        NodeKind::ClassExtendsClause => "ClassExtendsClause",
        NodeKind::TypeAlias => "TypeAlias",
        NodeKind::Annotation => "Annotation",
        NodeKind::ClassProperty => "ClassProperty",
        NodeKind::ClassMethod => "ClassMethod",
        NodeKind::ObjectProperty => "ObjectProperty",
        NodeKind::ObjectMethod => "ObjectMethod",
        NodeKind::MethodHeader => "MethodHeader",
        NodeKind::ParameterList => "ParameterList",
        NodeKind::TypeParameterList => "TypeParameterList",
        NodeKind::TypeParameter => "TypeParameter",
        NodeKind::TypedIdentifier => "TypedIdentifier",
        NodeKind::BlankIdentifier => "BlankIdentifier",
        NodeKind::TypeAnnotation => "TypeAnnotation",
        NodeKind::ArgumentList => "ArgumentList",
        NodeKind::ObjectBody => "ObjectBody",
        NodeKind::ObjectEntry => "ObjectEntry",
        NodeKind::ObjectElement => "ObjectElement",
        NodeKind::ObjectSpread => "ObjectSpread",
        NodeKind::MemberPredicate => "MemberPredicate",
        NodeKind::ForGenerator => "ForGenerator",
        NodeKind::WhenGenerator => "WhenGenerator",
        NodeKind::ThisExpr => "ThisExpr",
        NodeKind::OuterExpr => "OuterExpr",
        NodeKind::ModuleExpr => "ModuleExpr",
        NodeKind::NullLiteralExpr => "NullLiteralExpr",
        NodeKind::TrueLiteralExpr => "TrueLiteralExpr",
        NodeKind::FalseLiteralExpr => "FalseLiteralExpr",
        NodeKind::IntLiteralExpr => "IntLiteralExpr",
        NodeKind::FloatLiteralExpr => "FloatLiteralExpr",
        NodeKind::ThrowExpr => "ThrowExpr",
        NodeKind::TraceExpr => "TraceExpr",
        NodeKind::ImportExpr => "ImportExpr",
        NodeKind::ReadExpr => "ReadExpr",
        NodeKind::SingleLineStringLiteral => "SingleLineStringLiteral",
        NodeKind::MultiLineStringLiteral => "MultiLineStringLiteral",
        NodeKind::NewExpr => "NewExpr",
        NodeKind::AmendExpr => "AmendExpr",
        NodeKind::UnqualifiedAccessExpr => "UnqualifiedAccessExpr",
        NodeKind::QualifiedAccessExpr => "QualifiedAccessExpr",
        NodeKind::SuperAccessExpr => "SuperAccessExpr",
        NodeKind::SuperSubscriptExpr => "SuperSubscriptExpr",
        NodeKind::SubscriptExpr => "SubscriptExpr",
        NodeKind::NonNullExpr => "NonNullExpr",
        NodeKind::UnaryMinusExpr => "UnaryMinusExpr",
        NodeKind::LogicalNotExpr => "LogicalNotExpr",
        NodeKind::AdditiveExpr => "AdditiveExpr",
        NodeKind::MultiplicativeExpr => "MultiplicativeExpr",
        NodeKind::ComparisonExpr => "ComparisonExpr",
        NodeKind::EqualityExpr => "EqualityExpr",
        NodeKind::ExponentiationExpr => "ExponentiationExpr",
        NodeKind::LogicalAndExpr => "LogicalAndExpr",
        NodeKind::LogicalOrExpr => "LogicalOrExpr",
        NodeKind::NullCoalesceExpr => "NullCoalesceExpr",
        NodeKind::PipeExpr => "PipeExpr",
        NodeKind::TypeTestExpr => "TypeTestExpr",
        NodeKind::TypeCastExpr => "TypeCastExpr",
        NodeKind::IfExpr => "IfExpr",
        NodeKind::LetExpr => "LetExpr",
        NodeKind::FunctionLiteralExpr => "FunctionLiteralExpr",
        NodeKind::ParenthesizedExpr => "ParenthesizedExpr",
        NodeKind::UnknownType => "UnknownType",
        NodeKind::NothingType => "NothingType",
        NodeKind::ModuleType => "ModuleType",
        NodeKind::StringLiteralType => "StringLiteralType",
        NodeKind::DeclaredType => "DeclaredType",
        NodeKind::ParenthesizedType => "ParenthesizedType",
        NodeKind::NullableType => "NullableType",
        NodeKind::ConstrainedType => "ConstrainedType",
        NodeKind::UnionType => "UnionType",
        NodeKind::FunctionType => "FunctionType",
        NodeKind::DefaultUnionType => "DefaultUnionType",
        NodeKind::TypeArgumentList => "TypeArgumentList",
        NodeKind::QualifiedIdentifier => "QualifiedIdentifier",
        NodeKind::StringConstant => "StringConstant",
        NodeKind::StringInterpolation => "StringInterpolation",
        NodeKind::Error => "Error",
        NodeKind::LineComment => "LineComment",
        NodeKind::BlockComment => "BlockComment",
        NodeKind::ShebangComment => "ShebangComment",
        NodeKind::Terminal(_) => "Terminal",
        NodeKind::Unmapped(_) => "Unmapped",
    }
}

/// Strip surrounding backticks from a Pkl identifier text.
pub fn unquote_identifier(text: &str) -> &str {
    text.strip_prefix('`')
        .and_then(|s| s.strip_suffix('`'))
        .unwrap_or(text)
}
