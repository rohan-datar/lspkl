//! Pkl type system: resolved types, subtyping, type inference.

pub mod base_module;
mod compute_expr;
pub mod constraint;
pub mod context;
pub mod file_id;
mod infer_expr_type;
pub mod resolver;
mod to_type;
pub mod ty;

pub use base_module::{BaseModule, BaseModuleError};
pub use constraint::{CompareOp, ConstraintExpr, ConstraintValue};
pub use context::{AnalysisContext, ImportResolution};
pub use file_id::{FileId, InFile, NodeRef};
pub use ty::{Type, UnionMembers};

mod members;
pub use members::{MemberInfo, MemberKind, members_of_type};
pub use resolver::resolve_reference;

/// Resolve an expression in its pinned file.
pub fn expr_type(expr: InFile<pkl_syntax::ast::nodes::Expr>, ctx: &AnalysisContext) -> Type {
    compute_expr::compute_expr_type(expr.map(|expr| expr.id()), &ty::TypeBindings::new(), ctx)
}

/// Resolve `this` at a file-qualified syntax position.
pub fn this_type(node: NodeRef, ctx: &AnalysisContext) -> Type {
    compute_expr::compute_this_type(node, &ty::TypeBindings::new(), ctx)
}

/// Resolve the type demanded by an expression’s surrounding syntax.
pub fn expected_type(expr: InFile<pkl_syntax::ast::nodes::Expr>, ctx: &AnalysisContext) -> Type {
    infer_expr_type::infer_expr_type_from_context(
        expr.map(|expr| expr.id()),
        &ty::TypeBindings::new(),
        ctx,
    )
}

/// Find a member with its receiver’s generic arguments applied.
pub fn find_member(ty: &Type, name: &str, ctx: &AnalysisContext) -> Option<resolver::ResolvedType> {
    resolver::find_member_on_type(ty, name, &ty::TypeBindings::new(), ctx)
}

/// Convert a type node from its pinned file into a resolved type.
pub fn to_type(node: InFile<pkl_syntax::ast::nodes::TypeNode>, ctx: &AnalysisContext) -> Type {
    to_type::to_type_with_bindings(node.map(|node| node.id()), &ty::TypeBindings::new(), ctx)
}
