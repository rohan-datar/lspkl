//! Convert AST `PklType` syntax nodes into resolved [`Type`] values.

use pkl_syntax::ast::NodeKind;

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
                    .unwrap()
                    .name(type_node.node_id)
                    .unwrap()
                    .to_string(),
                constraints: Vec::new(),
            }
        }
        StringLiteralType => Type::string_literal(
            tree.escaped_string_literal(node.node_id)
                .expect("could not escape string literal text"),
        ),
        DeclaredType => {
            let Some(type_node) = resolve_declared_type(ctx, node) else {
                return Type::Unknown;
            };
            let Some(decl_tree) = ctx.tree_for(type_node.file_id) else {
                return Type::Unknown;
            };
            let type_args = decl_tree.child(type_node.node_id, |c| {
                matches!(c, NodeKind::TypeArgumentList)
            });
            let args = match type_args {
                Some(type_args) => decl_tree
                    .children_matching(type_args, |c| c.is_type_node())
                    .iter()
                    .map(|&n| to_type(type_node.with_node(n), bindings, ctx))
                    .collect(),

                None => vec![],
            };
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
                    reference_name: decl_tree
                        .name(type_node.node_id)
                        .expect("unable to find name for module")
                        .to_string(),
                    constraints: Vec::new(),
                },
                _ => Type::Unknown,
            }
        }
        ParenthesizedType | DefaultUnionType => to_type(
            node.with_node(tree.children(node.node_id)[0]),
            bindings,
            ctx,
        ),
        NullableType => to_type(
            node.with_node(tree.children(node.node_id)[0]),
            bindings,
            ctx,
        )
        .nullable(ctx),
        ConstrainedType => {
            let Some(inner) = tree.child(node.node_id, |c| c.is_type_node()) else {
                return Type::Unknown;
            };
            let inner_ty = to_type(node.with_node(inner), bindings, ctx);
            let cs = tree
                .children_matching(node.node_id, |c| c.is_expr_node())
                .iter()
                .map(|&n| ConstraintExpr::opaque(tree.text(n)))
                .collect();
            inner_ty.with_constraints(cs)
        }
        UnionType => {
            let type_children = tree.children_matching(node.node_id, |c| c.is_type_node());
            if type_children.len() != 2 {
                return Type::Unknown;
            }

            Type::Union {
                left: Box::new(to_type(node.with_node(type_children[0]), bindings, ctx)),
                right: Box::new(to_type(node.with_node(type_children[1]), bindings, ctx)),
                constraints: Vec::new(),
            }
        }
        FunctionType => {
            let types = tree.children_matching(node.node_id, |c| c.is_type_node());
            let n_params = types.len().saturating_sub(1);
            let return_type = to_type(
                node.with_node(*types.last().expect("Function must have return type")),
                bindings,
                ctx,
            );
            let param_nodes = &types[..types.len() - 1];
            let param_types: Vec<Type> = param_nodes
                .iter()
                .map(|&n| to_type(node.with_node(n), bindings, ctx))
                .collect();

            let args: Vec<Type> = param_types
                .iter()
                .cloned()
                .chain(std::iter::once(return_type))
                .collect();

            ctx.base
                .function_n_type(n_params)
                .unwrap_or(ctx.base.function_type().clone())
                .with_type_args(args)
        }
        _ => Type::Unknown,
    }
}
