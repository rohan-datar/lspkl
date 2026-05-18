//! Name resolution and class hierarchy queries.

use pkl_analysis::ast::{NodeId, NodeKind, SyntaxTree};

use crate::FileId;
use crate::compute_expr::compute_expr_type;
use crate::context::AnalysisContext;
use crate::file_id::NodeRef;
use crate::to_type::to_type;
use crate::ty::{Type, TypeBindings};

/// Look up a class by simple name in `tree`. Searches only the module's
/// top-level `Class` declarations — no aliases, no imports, no nested scope.
///
/// Returns the `NodeId` of the matching `NodeKind::Class` node, or `None` if
/// no class with that name exists at module level.
pub fn resolve_type_name_in_module(tree: &SyntaxTree, name: &str) -> Option<NodeId> {
    tree.children_matching(tree.root(), kind_is_class)
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

fn kind_is_class(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Class)
}

/// Look up a member by name on a type. Returns the member's resolved type if
/// found, or `None` otherwise.
pub fn find_member_on_type(
    ty: &Type,
    name: &str,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<Type> {
    match ty {
        Type::Class {
            class_node,
            type_args,
            ..
        } => {
            let tree = ctx.tree_for(class_node.file_id)?;
            let member_bindings = class_member_bindings(*class_node, type_args, bindings, ctx);
            if let Some(&class_member) = tree
                .children_matching(class_node.node_id, |k| {
                    matches!(k, NodeKind::ClassProperty | NodeKind::ClassMethod)
                })
                .iter()
                .find(|&&n| tree.name(n) == Some(name))
            {
                let type_annotation =
                    tree.child(class_member, |k| matches!(k, NodeKind::TypeAnnotation))?;
                Some(to_type(
                    NodeRef::new(class_node.file_id, type_annotation),
                    &member_bindings,
                    ctx,
                ))
            } else {
                let extends_target = tree.class_extends_target(class_node.node_id)?;
                let super_type_name = tree.qualified_identifier_segments(extends_target);
                let super_type_node = resolve_type_name(ctx, &super_type_name)?;
                find_member_on_type(
                    &to_type(super_type_node, &member_bindings, ctx),
                    name,
                    &member_bindings,
                    ctx,
                )
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
                let type_annotation =
                    tree.child(module_member, |k| matches!(k, NodeKind::TypeAnnotation))?;
                Some(to_type(
                    NodeRef::new(module_node.file_id, type_annotation),
                    bindings,
                    ctx,
                ))
            } else {
                let _super_module = tree.module_extends_or_amends_target()?;
                todo!("open the parent module and recurse")
            }
        }
        Type::Alias { .. } => find_member_on_type(&ty.unaliased(ctx), name, bindings, ctx),
        Type::Union { left, right, .. } => {
            let left_member = find_member_on_type(left, name, bindings, ctx);
            let Some(right_member) = find_member_on_type(right, name, bindings, ctx) else {
                return left_member;
            };
            if let Some(left_member) = left_member {
                Some(Type::Union {
                    left: Box::new(left_member),
                    right: Box::new(right_member),
                    constraints: vec![],
                })
            } else {
                Some(right_member)
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
) -> Option<Type> {
    let mut current = ctx.tree.parent(node);
    while let Some(id) = current {
        match ctx.tree.kind(id) {
            NodeKind::LetExpr => {
                let typed_identifier = ctx
                    .tree
                    .child(id, |k| matches!(k, NodeKind::TypedIdentifier))?;
                if ctx.tree.name(typed_identifier) != Some(name) {
                    current = ctx.tree.parent(id);
                    continue;
                }
                if let Some(explicit_anotation) = ctx
                    .tree
                    .child(typed_identifier, |k| matches!(k, NodeKind::TypedIdentifier))
                {
                    let type_node = ctx.tree.child(explicit_anotation, |k| k.is_type_node())?;
                    return Some(to_type(NodeRef::new(ctx.file_id, type_node), bindings, ctx));
                } else {
                    let expr = ctx.tree.child(typed_identifier, |k| k.is_expr_node())?;
                    return Some(compute_expr_type(expr, bindings, ctx));
                };
            }
            NodeKind::FunctionLiteralExpr => {
                let param_list = ctx
                    .tree
                    .child(id, |k| matches!(k, NodeKind::ParameterList))?;
                let &typed_identifier = ctx
                    .tree
                    .children_matching(param_list, |k| matches!(k, NodeKind::TypedIdentifier))
                    .iter()
                    .find(|&&n| ctx.tree.name(n) == Some(name))?;
                let annotation = ctx
                    .tree
                    .child(typed_identifier, |k| matches!(k, NodeKind::TypeAnnotation))?;
                let type_node = ctx.tree.child(annotation, |k| k.is_type_node())?;
                return Some(to_type(NodeRef::new(ctx.file_id, type_node), bindings, ctx));
            }
            NodeKind::ClassMethod => {
                let method_header = ctx
                    .tree
                    .child(id, |k| matches!(k, NodeKind::MethodHeader))?;
                let param_list = ctx
                    .tree
                    .child(method_header, |k| matches!(k, NodeKind::ParameterList))?;
                let &typed_identifier = ctx
                    .tree
                    .children_matching(param_list, |k| matches!(k, NodeKind::TypedIdentifier))
                    .iter()
                    .find(|&&n| ctx.tree.name(n) == Some(name))?;
                let annotation = ctx
                    .tree
                    .child(typed_identifier, |k| matches!(k, NodeKind::TypeAnnotation))?;
                let type_node = ctx.tree.child(annotation, |k| k.is_type_node())?;
                return Some(to_type(NodeRef::new(ctx.file_id, type_node), bindings, ctx));
            }
            _ => {
                current = ctx.tree.parent(id);
            }
        }
    }

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
