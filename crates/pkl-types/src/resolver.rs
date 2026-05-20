//! Name resolution and class hierarchy queries.

use pkl_analysis::ast::{NodeId, NodeKind, SyntaxTree};

use crate::FileId;
use crate::compute_expr::{compute_expr_type, compute_this_type};
use crate::context::AnalysisContext;
use crate::file_id::NodeRef;
use crate::to_type::to_type;
use crate::ty::{Type, TypeBindings};

#[derive(Debug)]
pub struct ResolvedType {
    pub node: NodeRef,
    pub ty: Type,
}

impl ResolvedType {
    fn new(node: NodeRef, ty: Type) -> Self {
        Self { node, ty }
    }
}

/// Look up a class by simple name in `tree`. Searches only the module's
/// top-level `Class` declarations — no aliases, no imports, no nested scope.
///
/// Returns the `NodeId` of the matching `NodeKind::Class` node, or `None` if
/// no class with that name exists at module level.
pub fn resolve_type_name_in_module(tree: &SyntaxTree, name: &str) -> Option<NodeId> {
    tree.children_matching(tree.root(), kind_is_class_or_alias)
        .iter()
        .find(|class| tree.name(**class) == Some(name))
        .cloned()
}

/// Look up the definition of a declared type
pub fn resolve_declared_type(ctx: &AnalysisContext, node: NodeRef) -> Option<NodeRef> {
    let tree = ctx.tree_for(node.file_id)?;
    let name_parts = tree.declared_type_parts(node.node_id);
    resolve_type_name(ctx, &name_parts)
}

// TODO: properly support qualified identifiers
pub fn resolve_type_name(ctx: &AnalysisContext, name_parts: &[&str]) -> Option<NodeRef> {
    let simple_name = name_parts.last()?;
    if let Some(type_def) = resolve_type_name_in_module(ctx.tree, simple_name) {
        return Some(NodeRef {
            file_id: ctx.file_id,
            node_id: type_def,
        });
    };
    if let Some(type_def) = resolve_type_name_in_module(ctx.base.tree(), simple_name) {
        return Some(NodeRef {
            file_id: FileId::BASE,
            node_id: type_def,
        });
    };
    None
}

/// Is class `a` a (non-strict) subclass of class `b`?
pub fn is_subclass(a: NodeRef, b: NodeRef, ctx: &AnalysisContext) -> bool {
    if a == b {
        return true;
    }

    let Some(a_tree) = ctx.tree_for(a.file_id) else {
        return false;
    };

    let Some(extends_target) = a_tree.class_extends_target(a.node_id) else {
        return false;
    };

    let super_type_name = a_tree.qualified_identifier_segments(extends_target);

    let Some(super_type_node) = resolve_type_name(ctx, &super_type_name) else {
        return false;
    };

    is_subclass(super_type_node, b, ctx)
}

fn kind_is_class_or_alias(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Class | NodeKind::TypeAlias)
}

/// Look up a member by name on a type. Returns the member's resolved type if
/// found, or `None` otherwise.
pub fn find_member_on_type(
    ty: &Type,
    name: &str,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<ResolvedType> {
    match ty {
        Type::Class {
            class_node,
            type_args,
            ..
        } => {
            let tree = ctx.tree_for(class_node.file_id)?;
            let member_bindings = class_member_bindings(*class_node, type_args, bindings, ctx);
            let member_parent = tree
                .child(class_node.node_id, |k| matches!(k, NodeKind::ClassBody))
                .unwrap_or(class_node.node_id);

            if let Some(&class_member) = tree
                .children_matching(member_parent, |k| {
                    matches!(k, NodeKind::ClassProperty | NodeKind::ClassMethod)
                })
                .iter()
                .find(|&&n| tree.name(n) == Some(name))
            {
                let annotation =
                    tree.child(class_member, |k| matches!(k, NodeKind::TypeAnnotation));
                let ty = annotation
                    .and_then(|ann| tree.child(ann, |k| k.is_type_node()))
                    .map(|ann| {
                        to_type(NodeRef::new(class_node.file_id, ann), &member_bindings, ctx)
                    })
                    .unwrap_or(Type::Unknown);
                let member_ref = NodeRef::new(class_node.file_id, class_member);

                Some(ResolvedType::new(member_ref, ty))
            } else {
                let extends_target = tree.class_extends_target(class_node.node_id)?;
                let super_type_name = tree.qualified_identifier_segments(extends_target);
                let super_type_node = resolve_type_name(ctx, &super_type_name)?;

                find_member_on_type(&Type::class(super_type_node), name, &member_bindings, ctx)
            }
        }
        Type::Module { module_node, .. } => {
            let tree = ctx.tree_for(module_node.file_id)?;
            if let Some(&module_member) = tree
                .children_matching(module_node.node_id, |k| {
                    matches!(
                        k,
                        NodeKind::ClassProperty
                            | NodeKind::ClassMethod
                            | NodeKind::Class
                            | NodeKind::TypeAlias
                    )
                })
                .iter()
                .find(|&&n| tree.name(n) == Some(name))
            {
                let annotation =
                    tree.child(module_member, |k| matches!(k, NodeKind::TypeAnnotation));
                let ty = annotation
                    .and_then(|ann| tree.child(ann, |k| k.is_type_node()))
                    .map(|ann| to_type(NodeRef::new(module_node.file_id, ann), bindings, ctx))
                    .unwrap_or(Type::Unknown);
                let member_ref = NodeRef::new(module_node.file_id, module_member);
                Some(ResolvedType::new(member_ref, ty))
            } else {
                let _super_module = tree.module_extends_or_amends_target()?;
                todo!("open the parent module and recurse")
            }
        }
        Type::Alias { .. } => find_member_on_type(&ty.unaliased(ctx), name, bindings, ctx),
        Type::Union { left, right, .. } => {
            let left_member = find_member_on_type(left, name, bindings, ctx);
            let right_member = find_member_on_type(right, name, bindings, ctx);
            match (left_member, right_member) {
                (
                    Some(ResolvedType {
                        node: l_ref,
                        ty: l_ty,
                    }),
                    Some(ResolvedType { ty: r_ty, .. }),
                ) => Some(ResolvedType::new(
                    l_ref,
                    Type::Union {
                        left: Box::new(l_ty),
                        right: Box::new(r_ty),
                        constraints: vec![],
                    },
                )),
                (Some(x), None) | (None, Some(x)) => Some(x),
                (None, None) => None,
            }
        }
        _ => None,
    }
}

/// Walk the AST ancestors of `node` looking for a binding named `name`.
pub fn resolve_local_binding(
    node: NodeId,
    name: &str,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<ResolvedType> {
    let mut current = ctx.tree.parent(node);
    while let Some(id) = current {
        let hit = match ctx.tree.kind(id) {
            NodeKind::LetExpr => match_let_binding(id, name, bindings, ctx),
            NodeKind::FunctionLiteralExpr => {
                let param_list = ctx.tree.child(id, |k| matches!(k, NodeKind::ParameterList));
                param_list.and_then(|pl| match_param(pl, name, bindings, ctx))
            }
            NodeKind::ClassMethod => {
                let param_list = ctx
                    .tree
                    .child(id, |k| matches!(k, NodeKind::MethodHeader))
                    .and_then(|mh| ctx.tree.child(mh, |k| matches!(k, NodeKind::ParameterList)));
                param_list.and_then(|pl| match_param(pl, name, bindings, ctx))
            }
            _ => None,
        };
        if hit.is_some() {
            return hit;
        }
        current = ctx.tree.parent(id);
    }
    None
}

/// If `let_node`'s TypedIdentifier matches `name`, return its
/// `ResolvedType`; otherwise `None`.
fn match_let_binding(
    let_node: NodeId,
    name: &str,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<ResolvedType> {
    let typed_identifier = ctx
        .tree
        .child(let_node, |k| matches!(k, NodeKind::TypedIdentifier))?;
    if ctx.tree.name(typed_identifier) != Some(name) {
        return None;
    }

    // Prefer the explicit annotation, fall back to inferring from the
    // initializer (LetExpr's first expr child), else Unknown.
    let ty_from_annotation = ctx
        .tree
        .child(typed_identifier, |k| matches!(k, NodeKind::TypeAnnotation))
        .and_then(|ann| ctx.tree.child(ann, |k| k.is_type_node()))
        .map(|tn| to_type(NodeRef::new(ctx.file_id, tn), bindings, ctx));

    let ty = ty_from_annotation.unwrap_or_else(|| {
        ctx.tree
            .children_matching(let_node, |k| k.is_expr_node())
            .first()
            .copied()
            .map(|expr| compute_expr_type(expr, bindings, ctx))
            .unwrap_or(Type::Unknown)
    });

    Some(ResolvedType::new(
        NodeRef::new(ctx.file_id, typed_identifier),
        ty,
    ))
}

/// If any `TypedIdentifier` child of `param_list` matches `name`, return its
/// `ResolvedType`.
fn match_param(
    param_list: NodeId,
    name: &str,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<ResolvedType> {
    let typed_identifier = ctx
        .tree
        .children_matching(param_list, |k| matches!(k, NodeKind::TypedIdentifier))
        .into_iter()
        .find(|&n| ctx.tree.name(n) == Some(name))?;

    let ty = ctx
        .tree
        .child(typed_identifier, |k| matches!(k, NodeKind::TypeAnnotation))
        .and_then(|ann| ctx.tree.child(ann, |k| k.is_type_node()))
        .map(|tn| to_type(NodeRef::new(ctx.file_id, tn), bindings, ctx))
        .unwrap_or(Type::Unknown);
    Some(ResolvedType::new(
        NodeRef::new(ctx.file_id, typed_identifier),
        ty,
    ))
}

pub fn resolve_unqualified_access(
    node: NodeId,
    name: &str,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<ResolvedType> {
    if let Some(local) = resolve_local_binding(node, name, bindings, ctx) {
        return Some(local);
    };
    if let Some(this) =
        find_member_on_type(&compute_this_type(node, bindings, ctx), name, bindings, ctx)
    {
        return Some(this);
    };
    if let Some(module_type) = find_member_on_type(
        &Type::Module {
            module_node: NodeRef::new(ctx.file_id, ctx.tree.root()),
            reference_name: "".to_string(), // this is probably fine, we don't really care about the module name
            constraints: vec![],
        },
        name,
        bindings,
        ctx,
    ) {
        return Some(module_type);
    };
    if let Some(base_type) = resolve_type_name_in_module(ctx.base.tree(), name) {
        return Some(ResolvedType::new(
            NodeRef::new(FileId::BASE, base_type),
            to_type(NodeRef::new(FileId::BASE, base_type), bindings, ctx),
        ));
    };

    None
}

pub fn class_member_bindings(
    class_node: NodeRef,
    type_args: &[Type],
    caller_bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> TypeBindings {
    let Some(tree) = ctx.tree_for(class_node.file_id) else {
        return caller_bindings.clone();
    };
    let mut member_bindings = caller_bindings.clone();
    if let Some(type_param_list) = tree.child(class_node.node_id, |k| {
        matches!(k, NodeKind::TypeParameterList)
    }) {
        let params =
            tree.children_matching(type_param_list, |k| matches!(k, NodeKind::TypeParameter));
        for (&param_node, arg) in params.iter().zip(type_args.iter()) {
            member_bindings.insert(NodeRef::new(class_node.file_id, param_node), arg.clone());
        }
    };
    member_bindings
}
