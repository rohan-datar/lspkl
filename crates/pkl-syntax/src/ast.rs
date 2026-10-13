//! Index-based syntax tree.
//!
//!  Every node lives in a `Vec` inside [`SyntaxTree`] and is
//! referenced by [`NodeId`].

use smallvec::SmallVec;

use crate::{LineIndex, TextRange, TextSize};
pub mod nodes;
// Typed views are implemented in the nodes submodule.
/// Opaque handle into a [`SyntaxTree`]'s node arena.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub(crate) u32);

impl NodeId {
    pub const ROOT: NodeId = NodeId(0);

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// All node kinds in the Pkl AST, mapped from tree-sitter's grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    Unmapped(u16),
}

impl NodeKind {
    pub fn name(&self) -> &str {
        match self {
            NodeKind::Terminal(_) => "Terminal",
            NodeKind::Unmapped(raw) => crate::parser::grammar()
                .node_kind_for_id(*raw)
                .unwrap_or("Unmapped"),
            other => kind_static_name(other),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, NodeKind::Terminal(_))
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
    Read,
    Amends,
    Extends,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    LogicalAnd,
    LogicalOr,
    NullCoalesce,
    Power,
    Pipe,
    Not,
    Arrow,

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
    pub span: TextRange,
    pub parent: Option<NodeId>,
    pub field: Option<std::num::NonZeroU16>,
    pub children: SmallVec<[NodeId; 6]>,
    pub is_missing: bool,
}

/// The complete syntax tree for one parsed file.
#[derive(Debug)]
pub struct SyntaxTree {
    nodes: Vec<NodeEntry>,
    source: std::sync::Arc<str>,
    line_index: std::sync::Arc<LineIndex>,
}

impl SyntaxTree {
    pub(crate) fn from_parts(
        source: std::sync::Arc<str>,
        nodes: Vec<NodeEntry>,
        line_index: std::sync::Arc<LineIndex>,
    ) -> Self {
        debug_assert!(!nodes.is_empty(), "tree must contain at least a root node");
        Self {
            nodes,
            source,
            line_index,
        }
    }

    pub fn root(&self) -> NodeId {
        NodeId::ROOT
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn line_index(&self) -> &LineIndex {
        &self.line_index
    }

    pub fn kind(&self, id: NodeId) -> &NodeKind {
        &self.nodes[id.index()].kind
    }

    pub fn span(&self, id: NodeId) -> TextRange {
        self.nodes[id.index()].span
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.index()].parent
    }

    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.nodes[id.index()].children
    }
    /// Find a direct child by its shared grammar field ID.
    pub fn child_by_field(&self, id: NodeId, name: &str) -> Option<NodeId> {
        let field = crate::parser::grammar().field_id_for_name(name)?;
        self.children(id)
            .iter()
            .copied()
            .find(|child| self.nodes[child.index()].field == Some(field))
    }

    pub fn is_missing(&self, id: NodeId) -> bool {
        self.nodes[id.index()].is_missing
    }

    pub fn text(&self, id: NodeId) -> &str {
        let span = self.span(id);
        &self.source[std::ops::Range::<usize>::from(span)]
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

    /// Find the deepest node containing the byte offset, including its end.
    /// Inclusive ends preserve cursor lookup at token boundaries and EOF.
    pub fn find_at(&self, offset: TextSize) -> Option<NodeId> {
        self.find_at_recursive(NodeId::ROOT, offset)
    }

    fn find_at_recursive(&self, id: NodeId, offset: TextSize) -> Option<NodeId> {
        for &child in self.children(id) {
            if self.span(child).contains_inclusive(offset) {
                return self.find_at_recursive(child, offset).or(Some(child));
            }
        }
        self.span(id).contains_inclusive(offset).then_some(id)
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

    pub fn doc_comments(&self, id: NodeId) -> Vec<&str> {
        self.children_matching(id, |k| {
            matches!(k, NodeKind::Terminal(TokenType::DocComment))
        })
        .iter()
        .map(|&c| self.text(c))
        .collect()
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

        nodes::ModuleExtendsAmendsClause::cast(self, header_clause)
            .is_some_and(|clause| clause.is_amend(self))
    }

    pub fn module_extends_or_amends_target(&self) -> Option<String> {
        let module = nodes::Module::cast(self, self.root())?;
        let header = nodes::ModuleHeader::cast(self, module.header(self)?)?;
        let clause =
            nodes::ModuleExtendsAmendsClause::cast(self, header.extends_amends_clause(self)?)?;
        let target = clause.target(self)?;
        self.escaped_string_literal(target)
    }

    pub fn escaped_string_literal(&self, string_node: NodeId) -> Option<String> {
        if !self.kind(string_node).is_string_literal() {
            return None;
        }

        decode_string_literal(self.text(string_node))
    }

    pub fn is_glob_import(&self, node: NodeId) -> bool {
        match self.kind(node) {
            NodeKind::ImportGlob => true,
            NodeKind::ImportExpr => self
                .child(node, |kind| {
                    matches!(kind, NodeKind::Terminal(TokenType::ImportGlob))
                })
                .is_some(),
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

/// Decode a complete Pkl string token without constructing a syntax tree.
pub fn decode_string_literal(literal: &str) -> Option<String> {
    let hashes = literal.bytes().take_while(|&b| b == b'#').count();
    let quoted = &literal[hashes..];
    let quotes = if quoted.starts_with("\"\"\"") { 3 } else { 1 };
    let end = literal.len().checked_sub(hashes + quotes)?;
    if !quoted.starts_with('"')
        || literal.get(end..end + quotes)?.bytes().any(|b| b != b'"')
        || literal[end + quotes..].bytes().any(|b| b != b'#')
    {
        return None;
    }
    let body = literal.get(hashes + quotes..end)?;
    let escape = "\\######".get(..hashes + 1)?;
    let mut result = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(index) = rest.find(escape) {
        result.push_str(&rest[..index]);
        rest = &rest[index + escape.len()..];
        let ch = rest.chars().next()?;
        rest = &rest[ch.len_utf8()..];
        match ch {
            'n' => result.push('\n'),
            'r' => result.push('\r'),
            't' => result.push('\t'),
            '"' => result.push('"'),
            '\\' => result.push('\\'),
            'u' => {
                rest = rest.strip_prefix('{')?;
                let close = rest.find('}')?;
                let digits = &rest[..close];
                if digits.is_empty() || digits.len() > 6 {
                    return None;
                }
                result.push(char::from_u32(u32::from_str_radix(digits, 16).ok()?)?);
                rest = &rest[close + 1..];
            }
            _ => return None,
        }
    }
    result.push_str(rest);
    Some(result)
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
