//! Per-declaration inference and cycle-safe semantic query entry points.
use crate::ty::TypeBindings;
use crate::{
    AnalysisContext, DeclId, InFile, NodeRef, Type, decl_for_node, item_node, item_source_map,
};
use lspkl_db::{Db, File};
use lspkl_syntax::ast::{NodeId, nodes};
use std::{collections::HashMap, sync::Arc};

/// Expression locations are local to this revision's declaration body;
/// semantic types themselves contain only stable declaration identities.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InferenceResult {
    pub expression_types: HashMap<NodeId, Type>,
}

#[salsa::interned(unsafe(no_lifetime), revisions = usize::MAX)]
struct BindingArguments {
    #[returns(ref)]
    values: TypeBindings,
}

#[salsa::interned(unsafe(no_lifetime), revisions = usize::MAX)]
struct DeclarationKey {
    #[returns(copy)]
    declaration: DeclId,
}

fn expression_cycle(_: &dyn Db, _: salsa::Id, _: NodeRef, _: File, _: BindingArguments) -> Type {
    Type::Unknown
}
#[salsa::tracked(returns(clone), cycle_result = expression_cycle)]
fn expression_query(db: &dyn Db, node: NodeRef, root: File, arguments: BindingArguments) -> Type {
    let ctx = AnalysisContext::for_root(db, root);
    crate::compute_expr::compute_expr_inner(node, arguments.values(db), &ctx)
}

/// Body-local dependencies use expression keys, not the containing body query:
/// reading another local value must not create a false whole-body cycle.
pub(crate) fn expression_type(
    db: &dyn Db,
    node: NodeRef,
    root: File,
    bindings: &TypeBindings,
) -> Type {
    expression_query(db, node, root, BindingArguments::new(db, bindings))
}

fn declaration_cycle(_: &dyn Db, _: salsa::Id, _: DeclarationKey) -> Type {
    Type::Unknown
}
#[salsa::tracked(returns(clone), cycle_result = declaration_cycle)]
fn declaration_type(db: &dyn Db, key: DeclarationKey) -> Type {
    crate::resolver::compute_decl_type(db, key.declaration(db))
}

pub fn decl_type(db: &dyn Db, declaration: DeclId) -> Type {
    declaration_type(db, DeclarationKey::new(db, declaration))
}
pub fn infer(db: &dyn Db, declaration: DeclId) -> &Arc<InferenceResult> {
    infer_body(db, DeclarationKey::new(db, declaration))
}

#[salsa::tracked(returns(ref))]
fn infer_body(db: &dyn Db, key: DeclarationKey) -> Arc<InferenceResult> {
    let declaration = key.declaration(db);
    let Some(owner) = item_node(db, declaration) else {
        return Arc::new(InferenceResult::default());
    };
    let ctx = AnalysisContext::for_root(db, declaration.root);
    let Some(tree) = ctx.tree(declaration.file) else {
        return Arc::new(InferenceResult::default());
    };
    let source_map = item_source_map(db, declaration.file);
    let mut pending = vec![owner.value];
    let mut expression_types = HashMap::new();
    let arguments = BindingArguments::new(db, TypeBindings::new());
    while let Some(node) = pending.pop() {
        db.unwind_if_revision_cancelled();
        if node != owner.value && source_map.declarations.contains_key(&node) {
            continue;
        }
        if nodes::Expr::can_cast(tree.kind(node)) {
            expression_types.insert(
                node,
                expression_query(db, owner.with_value(node), declaration.root, arguments),
            );
        }
        pending.extend(tree.children(node).iter().rev().copied());
    }
    Arc::new(InferenceResult { expression_types })
}

pub fn expr_type(db: &dyn Db, expr: InFile<nodes::Expr>, root: File) -> Type {
    let node = expr.map(|expr| expr.id());
    if let Some(declaration) = owner_for_node(db, node, root) {
        return infer(db, declaration)
            .expression_types
            .get(&node.value)
            .cloned()
            .unwrap_or(Type::Unknown);
    }
    expression_type(db, node, root, &TypeBindings::new())
}

/// Nearest lowered declaration, including class member bodies but excluding
/// position-dependent nested object members from global declaration identity.
fn owner_for_node(db: &dyn Db, node: NodeRef, root: File) -> Option<DeclId> {
    let ctx = AnalysisContext::for_root(db, root);
    let tree = ctx.tree(node.file)?;
    let mut current = Some(node.value);
    while let Some(id) = current {
        if let Some(declaration) = decl_for_node(db, node.with_value(id), root) {
            return Some(declaration);
        }
        current = tree.parent(id);
    }
    None
}

fn node_cycle(_: &dyn Db, _: salsa::Id, _: NodeRef, _: File) -> Type {
    Type::Unknown
}
#[salsa::tracked(returns(clone), cycle_result = node_cycle)]
pub fn this_type(db: &dyn Db, node: NodeRef, root: File) -> Type {
    let ctx = AnalysisContext::for_root(db, root);
    crate::compute_expr::compute_this_type_inner(node, &TypeBindings::new(), &ctx)
}
fn bound_node_cycle(_: &dyn Db, _: salsa::Id, _: NodeRef, _: File, _: BindingArguments) -> Type {
    Type::Unknown
}
#[salsa::tracked(returns(clone), cycle_result = bound_node_cycle)]
fn bound_this_type(db: &dyn Db, node: NodeRef, root: File, arguments: BindingArguments) -> Type {
    let ctx = AnalysisContext::for_root(db, root);
    crate::compute_expr::compute_this_type_inner(node, arguments.values(db), &ctx)
}
pub(crate) fn this_type_with_bindings(
    node: NodeRef,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Type {
    if bindings.is_empty() {
        this_type(ctx.db, node, ctx.root)
    } else {
        bound_this_type(
            ctx.db,
            node,
            ctx.root,
            BindingArguments::new(ctx.db, bindings),
        )
    }
}

#[salsa::tracked(returns(clone), cycle_result = node_cycle)]
pub fn expected_type(db: &dyn Db, node: NodeRef, root: File) -> Type {
    let ctx = AnalysisContext::for_root(db, root);
    crate::infer_expr_type::infer_expr_type_inner(node, &TypeBindings::new(), &ctx)
}
#[salsa::tracked(returns(clone), cycle_result = bound_node_cycle)]
fn bound_expected_type(
    db: &dyn Db,
    node: NodeRef,
    root: File,
    arguments: BindingArguments,
) -> Type {
    let ctx = AnalysisContext::for_root(db, root);
    crate::infer_expr_type::infer_expr_type_inner(node, arguments.values(db), &ctx)
}
pub(crate) fn expected_type_with_bindings(
    node: NodeRef,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Type {
    if bindings.is_empty() {
        expected_type(ctx.db, node, ctx.root)
    } else {
        bound_expected_type(
            ctx.db,
            node,
            ctx.root,
            BindingArguments::new(ctx.db, bindings),
        )
    }
}
