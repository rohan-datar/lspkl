//! Infer the *expected* type of an expression from its surrounding AST
//! context.

use pkl_syntax::ast::{NodeId, NodeKind};

use crate::compute_expr::{compute_expr_type, compute_this_type};
use crate::context::AnalysisContext;
use crate::file_id::NodeRef;
use crate::resolver::{
    access_receiver_type, container_key_type, container_value_type, find_member_on_type,
};
use crate::to_type::to_type;
use crate::ty::{Type, TypeBindings};

/// Given an expression node, return the type its position demands.
///
/// Returns [`Type::Unknown`] for any context that imposes no constraint —
/// the caller should treat that as "skip the type check".
pub fn infer_expr_type_from_context(
    node: NodeId,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Type {
    let Some(parent) = ctx.tree.parent(node) else {
        return Type::Unknown;
    };

    match ctx.tree.kind(parent) {
        // ===================================================================
        // Object body contexts
        // ===================================================================

        // `[key] = value` inside an ObjectBody.
        //
        // Children order: [keyExpr, valueExpr].
        //   - If `node` is the keyExpr: enclosing object's key type.
        //     For Mapping<K,V> → K; for Listing → Int.
        //   - If `node` is the valueExpr: enclosing object's value type.
        //     For Mapping<K,V> → V; for Listing<E> → E.
        NodeKind::ObjectEntry => {
            let this_type = compute_this_type(parent, bindings, ctx);
            let children = ctx.tree.children(parent);
            if children.len() != 2 {
                return Type::Unknown;
            }
            if node == children[0] {
                container_key_type(&this_type, ctx).unwrap_or(Type::Unknown)
            } else {
                container_value_type(&this_type, ctx).unwrap_or(Type::Unknown)
            }
        }

        // `...other` inside an ObjectBody. `other` should be an Object/Listing/
        // Mapping (whichever matches the enclosing this-type).
        NodeKind::ObjectSpread => compute_this_type(parent, bindings, ctx),

        // `[[predicate]] = value`. Children: [conditionExpr, valueExpr].
        //   - conditionExpr → Boolean
        //   - valueExpr     → element type of enclosing container
        NodeKind::MemberPredicate => {
            let children = ctx.tree.children(parent);
            if children.len() != 2 {
                return Type::Unknown;
            }
            if node == children[0] {
                ctx.base.boolean_type().clone()
            } else {
                let this_type = compute_this_type(parent, bindings, ctx);
                container_value_type(&this_type, ctx).unwrap_or(Type::Unknown)
            }
        }

        // `when (cond) { body }`. The cond child → Boolean.
        // Body's ObjectBody contents are checked via their own enclosing
        // context (the outer object), not via this arm.
        NodeKind::WhenGenerator => {
            let children = ctx.tree.children(parent);
            if children.is_empty() || children.len() != 2 {
                return Type::Unknown;
            }
            if node != children[0] {
                Type::Unknown
            } else {
                ctx.base.boolean_type().clone()
            }
        }

        // `for (binding in iterable) { body }`. Iterable expr position:
        //   - Listing<E>, List<E>, Set<E>, Mapping<K,V>, Map<K,V>, or IntRange
        NodeKind::ForGenerator => {
            let iterable = ctx
                .tree
                .children(parent)
                .iter()
                .find(|&&c| ctx.tree.kind(c).is_expr_node())
                .copied();
            if Some(node) == iterable {
                ctx.base.collection_type().clone()
            } else {
                Type::Unknown
            }
        }

        // ===================================================================
        // Call sites
        // ===================================================================

        // `foo(a, b, c)`. Resolve the surrounding access expr to find the
        // method's declared parameter type at this argument's position.
        NodeKind::ArgumentList => {
            infer_type_from_arg_list(node, parent, ctx, bindings).unwrap_or(Type::Unknown)
        }

        // `foo[idx]`. Receiver determines index type:
        //   - Listing<E> / List<E>: Int
        //   - Mapping<K,V> / Map<K,V>: K
        //   - String: Int
        //   - Else: Unknown
        NodeKind::SubscriptExpr => {
            let Some(receiver) = ctx.tree.child(parent, |k| k.is_expr_node()) else {
                return Type::Unknown;
            };
            let receiver_type = compute_expr_type(receiver, bindings, ctx);

            if receiver == node {
                return Type::Unknown;
            }
            if receiver_type.is_subtype_of(ctx.base.list_type(), ctx)
                || receiver_type.is_subtype_of(ctx.base.listing_type(), ctx)
                || receiver_type.is_subtype_of(ctx.base.string_type(), ctx)
            {
                ctx.base.int_type().clone()
            } else if receiver_type.is_subtype_of(ctx.base.map_type(), ctx)
                || receiver_type.is_subtype_of(ctx.base.mapping_type(), ctx)
            {
                let Type::Class { type_args, .. } = receiver_type else {
                    return Type::Unknown;
                };
                type_args.first().unwrap_or(&Type::Unknown).clone()
            } else {
                Type::Unknown
            }
        }

        // ===================================================================
        // Binary operators with restricted operand types
        // ===================================================================

        // `a ** b` — Number on both sides.
        NodeKind::ExponentiationExpr => ctx.base.number_type().clone(),

        // `a * b`, `a / b`, `a ~/ b`, `a % b` — Number-ish.
        NodeKind::MultiplicativeExpr => {
            let operands = operand_siblings(ctx, parent);
            let other = if node == operands[0] {
                operands[1]
            } else {
                operands[0]
            };
            let other_type = compute_expr_type(other, bindings, ctx);
            if other_type.is_subtype_of(ctx.base.duration_type(), ctx) {
                Type::Union {
                    left: Box::new(ctx.base.number_type().clone()),
                    right: Box::new(ctx.base.duration_type().clone()),
                    constraints: vec![],
                }
            } else if other_type.is_subtype_of(ctx.base.data_size_type(), ctx) {
                Type::Union {
                    left: Box::new(ctx.base.number_type().clone()),
                    right: Box::new(ctx.base.data_size_type().clone()),
                    constraints: vec![],
                }
            } else {
                // TODO: make this more exhaustive (support union types and return a pseudo type alias of multiplicative operands)
                ctx.base.number_type().clone()
            }
        }

        // `a + b`, `a - b` — Number, String, Bytes, List, Set, or Map
        // depending on the *other* operand's type.
        NodeKind::AdditiveExpr => {
            let operands = operand_siblings(ctx, parent);
            let other = if node == operands[0] {
                operands[1]
            } else {
                operands[0]
            };
            compute_expr_type(other, bindings, ctx)
        }

        // `a < b`, etc. — comparable: Number, Duration, DataSize, String, ...
        NodeKind::ComparisonExpr => ctx.base.comparable_type().clone(),

        // `a && b`, `a || b` — Boolean both sides.
        NodeKind::LogicalAndExpr | NodeKind::LogicalOrExpr => ctx.base.boolean_type().clone(),

        // `a |> b` — pipe. `b` must be a `(typeof(a)) -> R` function.
        //   - If `node` is the left side: Unknown (no constraint on input).
        NodeKind::PipeExpr => {
            let operands = operand_siblings(ctx, parent);
            let pos = operands.iter().position(|&c| c == node);
            if Some(1) == pos {
                let input_type = compute_expr_type(operands[0], bindings, ctx);
                let Some(fn1_type) = ctx.base.function_n_type(1) else {
                    return Type::Unknown;
                };
                fn1_type.with_type_args(vec![input_type, ctx.base.any_type().clone()])
            } else {
                Type::Unknown
            }
        }

        // ===================================================================
        // Unary operators
        // ===================================================================

        // `-x` — Number (Int or Float).
        NodeKind::UnaryMinusExpr => ctx.base.number_type().clone(),

        // `!x` — Boolean.
        NodeKind::LogicalNotExpr => ctx.base.boolean_type().clone(),

        // ===================================================================
        // Control flow / structural
        // ===================================================================

        // `if (c) t else e`. Children: [c, t, e].
        //   - c → Boolean
        //   - t, e → recurse on outer context (infer_expr_type_from_context(parent, ...))
        NodeKind::IfExpr => {
            if let Some(0) = ctx.tree.children(parent).iter().position(|&c| c == node) {
                ctx.base.boolean_type().clone()
            } else {
                infer_expr_type_from_context(parent, bindings, ctx)
            }
        }

        // `let (binding = init) body`. Children: [TypedIdentifier, init, body].
        //   - init → the TypedIdentifier's declared type (via to_type)
        //   - body → recurse on outer context
        NodeKind::LetExpr => {
            let children = ctx.tree.children(parent);
            if children.len() != 3 {
                return Type::Unknown;
            }

            if node == children[1] {
                let typed_ident = children[0];
                let Some(type_annotation) = ctx
                    .tree
                    .child(typed_ident, |k| matches!(k, NodeKind::TypeAnnotation))
                else {
                    return Type::Unknown;
                };
                let Some(type_node) = ctx.tree.child(type_annotation, |k| k.is_type_node()) else {
                    return Type::Unknown;
                };
                to_type(NodeRef::new(ctx.file_id, type_node), bindings, ctx)
            } else if node == children[2] {
                infer_expr_type_from_context(parent, bindings, ctx)
            } else {
                Type::Unknown
            }
        }

        // `(expr)` — fully transparent. Climb to grandparent's context.
        NodeKind::ParenthesizedExpr => infer_expr_type_from_context(parent, bindings, ctx),

        // ===================================================================
        // Built-in functions taking a string
        // ===================================================================

        // `read("uri")` — argument is a String URI.
        NodeKind::ReadExpr => ctx.base.string_type().clone(),

        // `throw("message")` — argument is a String message.
        NodeKind::ThrowExpr => ctx.base.string_type().clone(),

        // ===================================================================
        // Fallback: declared-type extraction from parent.
        // Covers:
        //   - ClassProperty:    parent.TypeAnnotation → declared type of `x: T`
        //   - ObjectProperty:   same
        //   - ClassMethod:      MethodHeader.TypeAnnotation → return type
        //   - ObjectMethod:     same
        //   - TypeCastExpr:     the cast target type IS the operand's expected
        //                       type? No — operand is unconstrained, the cast
        //                       narrows it. So this falls through to Unknown.
        //   - Other nodes carrying a type child: return that child's type.
        //
        // Anything else → Unknown.
        // ===================================================================
        _ => parent_declared_type(parent, bindings, ctx).unwrap_or(Type::Unknown),
    }
}

/// The operand expressions of a binary-expression `parent`, in source order.
fn operand_siblings(ctx: &AnalysisContext, parent: NodeId) -> Vec<NodeId> {
    ctx.tree
        .children(parent)
        .iter()
        .copied()
        .filter(|&c| !matches!(ctx.tree.kind(c), NodeKind::Terminal(_)))
        .collect()
}

/// Find the type annotation attached to a *member-like* parent
fn parent_declared_type(
    parent: NodeId,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<Type> {
    let _ = (parent, bindings, ctx);
    let wrapper = if matches!(
        ctx.tree.kind(parent),
        NodeKind::ClassMethod | NodeKind::ObjectMethod
    ) {
        ctx.tree
            .child(parent, |k| matches!(k, NodeKind::MethodHeader))?
    } else {
        parent
    };
    let annotation = ctx
        .tree
        .child(wrapper, |k| matches!(k, NodeKind::TypeAnnotation))?;
    let type_node = ctx.tree.child(annotation, |k| k.is_type_node())?;

    Some(to_type(NodeRef::new(ctx.file_id, type_node), bindings, ctx))
}

fn infer_type_from_arg_list(
    node: NodeId,
    parent: NodeId,
    ctx: &AnalysisContext,
    bindings: &TypeBindings,
) -> Option<Type> {
    let access = ctx.tree.parent(parent)?;
    let receiver_type = access_receiver_type(access, bindings, ctx)?;
    let name = ctx.tree.name(access).unwrap_or("");
    let method = find_member_on_type(&receiver_type, name, bindings, ctx)?;
    let arg_index = ctx.tree.children(parent).iter().position(|&c| c == node)?;
    let tree = ctx.tree_for(method.node.file_id)?;
    let header = tree.child(method.node.node_id, |k| matches!(k, NodeKind::MethodHeader))?;
    let param_list = tree.child(header, |k| matches!(k, NodeKind::ParameterList))?;
    let &param_decl = tree.children(param_list).get(arg_index)?;
    let type_annotation = tree.child(param_decl, |k| matches!(k, NodeKind::TypeAnnotation))?;
    Some(to_type(
        NodeRef::new(method.node.file_id, type_annotation),
        bindings,
        ctx,
    ))
}
