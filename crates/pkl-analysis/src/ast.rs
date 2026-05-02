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

/// 1-indexed line/column source span. Matches pkl-lsp's `Span` for snapshot parity.
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
    ImportGlobExpr,
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
    TypeName,
    SimpleTypeName,
    ModuleName,

    // ---- Other ----
    QualifiedIdentifier,
    StringConstant,
    StringInterpolation,
    ModuleUri,
    Terminal(TokenType),
    Error,
    LineComment,
    BlockComment,
    ShebangComment,

    /// A tree-sitter node type we have not mapped to a concrete variant yet.
    /// Carries the raw tree-sitter type string so snapshots reveal what is missing.
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
        self.child(id, |k| {
            matches!(k, NodeKind::Terminal(TokenType::Identifier))
        })
    }

    pub fn name(&self, id: NodeId) -> Option<&str> {
        self.identifier(id).map(|nid| self.text(nid))
    }

    pub fn modifiers(&self, id: NodeId) -> Vec<TokenType> {
        self.children(id)
            .iter()
            .copied()
            .filter_map(|c| match self.kind(c) {
                NodeKind::Terminal(tt) if tt.is_modifier() => Some(*tt),
                _ => None,
            })
            .collect()
    }

    pub fn has_modifier(&self, id: NodeId, modifier: TokenType) -> bool {
        self.modifiers(id).contains(&modifier)
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
        NodeKind::ImportGlobExpr => "ImportGlobExpr",
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
        NodeKind::TypeName => "TypeName",
        NodeKind::SimpleTypeName => "SimpleTypeName",
        NodeKind::ModuleName => "ModuleName",
        NodeKind::QualifiedIdentifier => "QualifiedIdentifier",
        NodeKind::StringConstant => "StringConstant",
        NodeKind::StringInterpolation => "StringInterpolation",
        NodeKind::ModuleUri => "ModuleUri",
        NodeKind::Error => "Error",
        NodeKind::LineComment => "LineComment",
        NodeKind::BlockComment => "BlockComment",
        NodeKind::ShebangComment => "ShebangComment",
        NodeKind::Terminal(_) => "Terminal",
        NodeKind::Unmapped(_) => "Unmapped",
    }
}
