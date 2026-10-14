//! Typed, allocation-free views over grammar nodes. Positional grammar knowledge lives here only.
use super::{NodeId, NodeKind, SyntaxTree, TokenType};

macro_rules! ast_node { ($($name:ident),* $(,)?) => {$ (
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct $name(NodeId);
    impl $name {
        pub fn cast(tree: &SyntaxTree, id: NodeId) -> Option<Self> { matches!(tree.kind(id), NodeKind::$name).then_some(Self(id)) }
        pub fn id(self) -> NodeId { self.0 }
    }
)*}; }
ast_node!(
    Module,
    ModuleHeader,
    ModuleClause,
    ModuleExtendsAmendsClause,
    Import,
    ImportGlob,
    Class,
    ClassBody,
    ClassExtendsClause,
    TypeAlias,
    Annotation,
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
    ObjectBody,
    ObjectEntry,
    ObjectElement,
    ObjectSpread,
    MemberPredicate,
    ForGenerator,
    WhenGenerator,
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
    QualifiedIdentifier,
    StringConstant,
    StringInterpolation
);

macro_rules! category { ($name:ident: $($variant:ident),* $(,)?) => {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum $name { $($variant($variant)),* }
    impl $name {
        pub fn can_cast(kind: &NodeKind) -> bool { matches!(kind, $(NodeKind::$variant)|*) }
        pub fn cast(tree: &SyntaxTree, id: NodeId) -> Option<Self> { match tree.kind(id) { $(NodeKind::$variant => Some(Self::$variant($variant(id))),)* _ => None } }
        pub fn id(self) -> NodeId { match self { $(Self::$variant(node) => node.id()),* } }
        pub fn children(tree: &SyntaxTree, parent: NodeId) -> impl Iterator<Item = NodeId> + '_ { tree.children(parent).iter().copied().filter(|id| Self::can_cast(tree.kind(*id))) }
    }
}; }
category!(Expr: ThisExpr, OuterExpr, ModuleExpr, NullLiteralExpr, TrueLiteralExpr, FalseLiteralExpr, IntLiteralExpr, FloatLiteralExpr, ThrowExpr, TraceExpr, ImportExpr, ReadExpr, SingleLineStringLiteral, MultiLineStringLiteral, NewExpr, AmendExpr, UnqualifiedAccessExpr, QualifiedAccessExpr, SuperAccessExpr, SuperSubscriptExpr, SubscriptExpr, NonNullExpr, UnaryMinusExpr, LogicalNotExpr, AdditiveExpr, MultiplicativeExpr, ComparisonExpr, EqualityExpr, ExponentiationExpr, LogicalAndExpr, LogicalOrExpr, NullCoalesceExpr, PipeExpr, TypeTestExpr, TypeCastExpr, IfExpr, LetExpr, FunctionLiteralExpr, ParenthesizedExpr);
category!(TypeNode: UnknownType, NothingType, ModuleType, StringLiteralType, DeclaredType, ParenthesizedType, NullableType, ConstrainedType, UnionType, FunctionType, DefaultUnionType);
category!(ObjectMember: ObjectProperty, ObjectMethod, ObjectEntry, ObjectElement, MemberPredicate, ForGenerator, WhenGenerator, ObjectSpread);
category!(ModuleMember: Class, TypeAlias, ClassProperty, ClassMethod);

mod common;
mod declarations;
mod expressions;
mod object_members;
mod types;
pub use common::*;
pub use declarations::Variance;
pub use expressions::*;
