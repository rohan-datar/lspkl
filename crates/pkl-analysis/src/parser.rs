//! tree-sitter wrapper that turns Pkl source into a [`SyntaxTree`].
//!
//! The parser does two things:
//!   1. Run tree-sitter-pkl over the source.
//!   2. Walk the tree-sitter CST and project it into our index-based AST,
//!      mapping each tree-sitter node `kind` string to a [`NodeKind`] variant.
//!
//! Anything we have not mapped yet falls into [`NodeKind::Unmapped`] so the
//! parser never panics on unknown nodes.

use smallvec::SmallVec;
use thiserror::Error;
use tree_sitter::{Node as TsNode, Parser};

use crate::ast::{NodeEntry, NodeId, NodeKind, Span, SyntaxTree, TokenType};

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("failed to load tree-sitter-pkl grammar: {0}")]
    Language(String),

    #[error("tree-sitter halted parsing")]
    Halted,
}

/// Parse a Pkl source string into a [`SyntaxTree`].
pub fn parse(source: &str) -> Result<SyntaxTree, ParseError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_pkl::LANGUAGE.into())
        .map_err(|e| ParseError::Language(e.to_string()))?;

    let ts_tree = parser.parse(source, None).ok_or(ParseError::Halted)?;

    let mut builder = TreeBuilder::new();
    builder.visit(ts_tree.root_node(), None);
    Ok(SyntaxTree::from_parts(source.to_string(), builder.nodes))
}

/// Build the index-based AST from a tree-sitter tree, depth-first.
struct TreeBuilder {
    nodes: Vec<NodeEntry>,
}

impl TreeBuilder {
    fn new() -> Self {
        Self {
            nodes: Vec::with_capacity(64),
        }
    }

    fn visit(&mut self, ts_node: TsNode<'_>, parent: Option<NodeId>) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        let kind = ts_node_to_kind(&ts_node);
        let span = span_of(&ts_node);
        let is_missing = ts_node.is_missing();

        // Reserve the slot now so child visitations see the right ids.
        self.nodes.push(NodeEntry {
            kind,
            span,
            parent,
            children: SmallVec::new(),
            is_missing,
        });

        let mut child_ids: SmallVec<[NodeId; 6]> = SmallVec::new();
        let mut cursor = ts_node.walk();
        for child in ts_node.children(&mut cursor) {
            if !should_keep_child(&child) {
                continue;
            }
            child_ids.push(self.visit(child, Some(id)));
        }
        self.nodes[id.index()].children = child_ids;
        id
    }
}

/// Skip whitespace and other non-semantic tokens. Tree-sitter calls these
/// "extras" and they appear as anonymous nodes; we don't want them in the AST.
///
/// TODO: refine — comments belong in the AST (LineComment/BlockComment), but
/// pure whitespace does not. For now we keep all named nodes plus ERROR.
fn should_keep_child(ts_node: &TsNode<'_>) -> bool {
    ts_node.is_named() || ts_node.is_error() || ts_node.is_missing()
}

/// Convert a tree-sitter node to its [`NodeKind`].
fn ts_node_to_kind(ts_node: &TsNode<'_>) -> NodeKind {
    let raw = ts_node.kind();
    match raw {
        // ---- Top-level ----
        "module" => NodeKind::Module,
        "moduleHeader" => NodeKind::ModuleHeader,
        "moduleClause" => NodeKind::ModuleClause,
        "extendsOrAmendsClause" => NodeKind::ModuleExtendsAmendsClause,
        "importClause" => NodeKind::Import,
        "importGlobClause" => NodeKind::ImportGlob,

        // ---- Declarations ----
        "clazz" => NodeKind::Class,
        "classBody" => NodeKind::ClassBody,
        "classExtendsClause" => NodeKind::ClassExtendsClause,
        "typeAlias" => NodeKind::TypeAlias,
        "annotation" => NodeKind::Annotation,

        // ---- Members ----
        "classProperty" => NodeKind::ClassProperty,
        "classMethod" => NodeKind::ClassMethod,
        "objectProperty" => NodeKind::ObjectProperty,
        "objectMethod" => NodeKind::ObjectMethod,
        "methodHeader" => NodeKind::MethodHeader,
        "parameterList" | "objectBodyParameters" => NodeKind::ParameterList,
        "typeParameterList" => NodeKind::TypeParameterList,
        "typeParameter" => NodeKind::TypeParameter,
        "typedIdentifier" => NodeKind::TypedIdentifier,
        "blankIdentifier" => NodeKind::BlankIdentifier,
        "typeAnnotation" => NodeKind::TypeAnnotation,
        "argumentList" => NodeKind::ArgumentList,

        // ---- Object body ----
        "objectBody" => NodeKind::ObjectBody,
        "objectEntry" => NodeKind::ObjectEntry,
        "objectElement" => NodeKind::ObjectElement,
        "objectSpread" => NodeKind::ObjectSpread,
        "memberPredicate" => NodeKind::MemberPredicate,
        "forGenerator" => NodeKind::ForGenerator,
        "whenGenerator" => NodeKind::WhenGenerator,

        // ---- Expressions ----
        "thisExpr" => NodeKind::ThisExpr,
        "outerExpr" => NodeKind::OuterExpr,
        "moduleExpr" => NodeKind::ModuleExpr,
        "nullLiteralExpr" => NodeKind::NullLiteralExpr,
        "trueLiteralExpr" => NodeKind::TrueLiteralExpr,
        "falseLiteralExpr" => NodeKind::FalseLiteralExpr,
        "intLiteralExpr" => NodeKind::IntLiteralExpr,
        "floatLiteralExpr" => NodeKind::FloatLiteralExpr,
        "throwExpr" => NodeKind::ThrowExpr,
        "traceExpr" => NodeKind::TraceExpr,
        "importExpr" => NodeKind::ImportExpr,
        "importGlobExpr" => NodeKind::ImportGlobExpr,
        "readExpr" => NodeKind::ReadExpr,
        "slStringLiteralExpr" => NodeKind::SingleLineStringLiteral,
        "mlStringLiteralExpr" => NodeKind::MultiLineStringLiteral,
        "newExpr" => NodeKind::NewExpr,
        "amendExpr" => NodeKind::AmendExpr,
        "unqualifiedAccessExpr" => NodeKind::UnqualifiedAccessExpr,
        "qualifiedAccessExpr" => NodeKind::QualifiedAccessExpr,
        "superAccessExpr" => NodeKind::SuperAccessExpr,
        "superSubscriptExpr" => NodeKind::SuperSubscriptExpr,
        "subscriptExpr" => NodeKind::SubscriptExpr,
        "nonNullExpr" => NodeKind::NonNullExpr,
        "unaryMinusExpr" => NodeKind::UnaryMinusExpr,
        "logicalNotExpr" => NodeKind::LogicalNotExpr,
        "additiveExpr" => NodeKind::AdditiveExpr,
        "multiplicativeExpr" => NodeKind::MultiplicativeExpr,
        "comparisonExpr" => NodeKind::ComparisonExpr,
        "equalityExpr" => NodeKind::EqualityExpr,
        "exponentiationExpr" => NodeKind::ExponentiationExpr,
        "logicalAndExpr" => NodeKind::LogicalAndExpr,
        "logicalOrExpr" => NodeKind::LogicalOrExpr,
        "nullCoalesceExpr" => NodeKind::NullCoalesceExpr,
        "pipeExpr" => NodeKind::PipeExpr,
        "typeTestExpr" => NodeKind::TypeTestExpr,
        "typeCastExpr" => NodeKind::TypeCastExpr,
        "ifExpr" => NodeKind::IfExpr,
        "letExpr" => NodeKind::LetExpr,
        "functionLiteralExpr" => NodeKind::FunctionLiteralExpr,
        "parenthesizedExpr" => NodeKind::ParenthesizedExpr,

        // ---- Type annotations ----
        "unknownType" => NodeKind::UnknownType,
        "nothingType" => NodeKind::NothingType,
        "moduleType" => NodeKind::ModuleType,
        "stringLiteralType" => NodeKind::StringLiteralType,
        "declaredType" => NodeKind::DeclaredType,
        "parenthesizedType" => NodeKind::ParenthesizedType,
        "nullableType" => NodeKind::NullableType,
        "constrainedType" => NodeKind::ConstrainedType,
        "unionType" => NodeKind::UnionType,
        "functionLiteralType" => NodeKind::FunctionType,
        "defaultUnionType" => NodeKind::DefaultUnionType,
        "typeArgumentList" => NodeKind::TypeArgumentList,
        "typeName" => NodeKind::TypeName,
        "simpleTypeName" => NodeKind::SimpleTypeName,
        "moduleName" => NodeKind::ModuleName,

        // ---- Other ----
        "qualifiedIdentifier" => NodeKind::QualifiedIdentifier,
        "stringConstant" => NodeKind::StringConstant,
        "stringInterpolation" => NodeKind::StringInterpolation,
        "moduleUri" => NodeKind::ModuleUri,
        "lineComment" => NodeKind::LineComment,
        "blockComment" => NodeKind::BlockComment,
        "shebangComment" => NodeKind::ShebangComment,
        "ERROR" => NodeKind::Error,

        // ---- Terminals ----
        "identifier" => NodeKind::Terminal(TokenType::Identifier),
        "docComment" => NodeKind::Terminal(TokenType::DocComment),
        "modifier" => NodeKind::Terminal(modifier_token(ts_node)),
        "slStringLiteralPart" => NodeKind::Terminal(TokenType::StringLiteral),

        // Unknown — log via the snapshot output.
        other => NodeKind::Unmapped(other.to_string()),
    }
}

/// tree-sitter-pkl wraps modifier keywords inside a `modifier` node whose only
/// child is the keyword itself. Depending on the grammar that child may be
/// named or anonymous, so we look at `named_child(0)` first and fall back to
/// the very first child of any kind.
fn modifier_token(ts_node: &TsNode<'_>) -> TokenType {
    let mut cursor = ts_node.walk();
    let kind = ts_node
        .named_child(0)
        .or_else(|| ts_node.children(&mut cursor).next())
        .map(|child| child.kind());

    match kind {
        Some("abstract") => TokenType::Abstract,
        Some("external") => TokenType::External,
        Some("hidden") => TokenType::Hidden,
        Some("local") => TokenType::Local,
        Some("open") => TokenType::Open,
        Some("fixed") => TokenType::Fixed,
        Some("const") => TokenType::Const,
        _ => TokenType::Other,
    }
}

fn span_of(ts_node: &TsNode<'_>) -> Span {
    let start = ts_node.start_position();
    let end = ts_node.end_position();
    Span {
        // tree-sitter rows/columns are 0-indexed; Kotlin Span is 1-indexed.
        begin_line: (start.row as u32).saturating_add(1),
        begin_col: (start.column as u32).saturating_add(1),
        end_line: (end.row as u32).saturating_add(1),
        end_col: (end.column as u32).saturating_add(1),
        begin_byte: ts_node.start_byte() as u32,
        end_byte: ts_node.end_byte() as u32,
    }
}
