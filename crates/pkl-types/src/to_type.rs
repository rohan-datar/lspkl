//! Convert AST `PklType` syntax nodes into resolved [`Type`] values.

use pkl_syntax::ast::NodeKind;
use pkl_syntax::ast::nodes;

use crate::ConstraintExpr;

use crate::NodeRef;
use crate::context::AnalysisContext;

use crate::resolver::resolve_declared_type;

use crate::ty::{Type, TypeBindings};

/// Convert an AST type node into a resolved [`Type`].
///
/// `node` must point at a node whose `NodeKind` is one of the type variants
/// (`DeclaredType`, `NullableType`, `UnionType`, ...). Returns
/// [`Type::Unknown`] on unresolved names — never an error — to avoid
/// cascading diagnostics.
pub fn to_type(node: NodeRef, bindings: &TypeBindings, ctx: &AnalysisContext) -> Type {
    use NodeKind::*;
    let Some(tree) = ctx.tree_for(node.file_id) else {
        return Type::Unknown;
    };
    match tree.kind(node.node_id) {
        UnknownType => Type::Unknown,
        NothingType => Type::Nothing,
        ModuleType => {
            let Some(type_node) = resolve_declared_type(ctx, node) else {
                return ctx.base.module_type().clone();
            };

            Type::Module {
                module_node: type_node,
                reference_name: ctx
                    .tree_for(type_node.file_id)
                    .and_then(|t| t.name(type_node.node_id))
                    .unwrap_or("")
                    .to_string(),
                constraints: Vec::new(),
            }
        }
        StringLiteralType => tree
            .escaped_string_literal(node.node_id)
            .map(Type::string_literal)
            .unwrap_or(Type::Unknown),
        DeclaredType => {
            let Some(type_node) = resolve_declared_type(ctx, node) else {
                return Type::Unknown;
            };
            let Some(decl_tree) = ctx.tree_for(type_node.file_id) else {
                return Type::Unknown;
            };
            let Some(declared) = nodes::DeclaredType::cast(tree, node.node_id) else {
                return Type::Unknown;
            };
            let args = declared
                .arguments(tree)
                .map(|n| to_type(node.with_node(n), bindings, ctx))
                .collect();
            match decl_tree.kind(type_node.node_id) {
                Class => Type::class_with_args(type_node, args),
                TypeAlias => Type::Alias {
                    alias_node: type_node,
                    type_args: args,
                    constraints: Vec::new(),
                },
                TypeParameter => bindings.get(&type_node).cloned().unwrap_or(Type::Unknown),
                Module => Type::Module {
                    module_node: type_node,
                    reference_name: decl_tree.name(type_node.node_id).unwrap_or("").to_string(),
                    constraints: Vec::new(),
                },
                _ => Type::Unknown,
            }
        }
        ParenthesizedType | DefaultUnionType | NullableType => {
            let inner = nodes::ParenthesizedType::cast(tree, node.node_id)
                .and_then(|n| n.inner(tree))
                .or_else(|| {
                    nodes::DefaultUnionType::cast(tree, node.node_id).and_then(|n| n.inner(tree))
                })
                .or_else(|| {
                    nodes::NullableType::cast(tree, node.node_id).and_then(|n| n.inner(tree))
                });
            let Some(inner) = inner else {
                return Type::Unknown;
            };
            let ty = to_type(node.with_node(inner), bindings, ctx);
            if matches!(tree.kind(node.node_id), NullableType) {
                ty.nullable(ctx)
            } else {
                ty
            }
        }
        ConstrainedType => {
            let Some(constrained) = nodes::ConstrainedType::cast(tree, node.node_id) else {
                return Type::Unknown;
            };
            let Some(inner) = constrained.inner(tree) else {
                return Type::Unknown;
            };
            let inner_ty = to_type(node.with_node(inner), bindings, ctx);
            let cs = constrained
                .constraints(tree)
                .map(|n| ConstraintExpr::opaque(tree.text(n)))
                .collect();
            inner_ty.with_constraints(cs)
        }
        UnionType => {
            let Some(union) = nodes::UnionType::cast(tree, node.node_id) else {
                return Type::Unknown;
            };
            let (Some(left), Some(right)) = (union.left(tree), union.right(tree)) else {
                return Type::Unknown;
            };
            Type::Union {
                left: Box::new(to_type(node.with_node(left), bindings, ctx)),
                right: Box::new(to_type(node.with_node(right), bindings, ctx)),
                constraints: Vec::new(),
            }
        }
        FunctionType => {
            let Some(function) = nodes::FunctionType::cast(tree, node.node_id) else {
                return Type::Unknown;
            };
            let Some(return_node) = function.return_type(tree) else {
                return Type::Unknown;
            };
            let mut args: Vec<Type> = function
                .parameters(tree)
                .map(|n| to_type(node.with_node(n), bindings, ctx))
                .collect();
            let n_params = args.len();
            args.push(to_type(node.with_node(return_node), bindings, ctx));

            ctx.base
                .function_n_type(n_params)
                .unwrap_or(ctx.base.function_type().clone())
                .with_type_args(args)
        }
        _ => Type::Unknown,
    }
}
