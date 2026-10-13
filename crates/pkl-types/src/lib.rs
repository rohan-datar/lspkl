//! Pkl type system: resolved types, subtyping, and memoized type inference.

pub mod base_module;
mod compute_expr;
pub mod constraint;
pub mod context;
pub mod file_id;
mod infer_expr_type;
mod inference;
pub mod items;
pub mod member_queries;
pub mod resolver;
pub mod scope;
mod to_type;
pub mod ty;
mod type_resolution;

pub use base_module::{BaseModule, BaseModuleError, base_module};
pub use constraint::{CompareOp, ConstraintExpr, ConstraintValue};
pub use context::{AnalysisContext, ImportResolution};
pub use file_id::{InFile, NodeRef};
pub use inference::{InferenceResult, decl_type, expected_type, expr_type, infer, this_type};
pub use items::{
    DeclId, ItemId, ItemSourceMap, Modifiers, ModuleItems, Name, TypeParameterId, TypeRef,
    decl_for_node, declaration_kind, import_declarations, item_node, item_source_map, module_items,
};
pub use member_queries::{
    ClassId, MemberDecl, MemberTable, ModuleId, class_id, class_members, module_id, module_members,
};
pub use scope::{Scope, ScopeEntry, ScopeKind, scope_at};
pub use ty::{Type, UnionMembers};
pub use type_resolution::{resolve_alias, resolve_type_ref};

mod members;
pub use members::{MemberInfo, MemberKind, members_of_type};
pub use resolver::resolve_reference;

/// Find a member with its receiver's generic arguments applied.
pub fn find_member(db: &dyn pkl_db::Db, ty: &Type, name: &str) -> Option<resolver::ResolvedType> {
    let ctx = AnalysisContext::for_root(db, ty.context_root(db));
    resolver::find_member_on_type(ty, name, &ty::TypeBindings::new(), &ctx)
}

/// Resolve an annotation through the lowered type-reference query.
pub fn to_type(
    db: &dyn pkl_db::Db,
    node: InFile<pkl_syntax::ast::nodes::TypeNode>,
    root: pkl_db::File,
) -> Type {
    let ctx = AnalysisContext::for_root(db, root);
    to_type::to_type_with_bindings(node.map(|node| node.id()), &ty::TypeBindings::new(), &ctx)
}
