//! Infer the *expected* type of an expression from its surrounding AST
//! context.

use lspkl_syntax::ast::nodes;
use lspkl_syntax::ast::{NodeId, NodeKind};

use crate::compute_expr::{compute_expr_type, compute_this_type};
use crate::context::AnalysisContext;
use crate::file_id::NodeRef;
use crate::resolver::{
    access_receiver_type_with_bindings, container_key_type, container_value_type,
    find_member_on_type,
};
use crate::to_type::to_type_with_bindings;
use crate::ty::{Type, TypeBindings};

/// Given an expression node, return the type its position demands.
///
/// Returns [`Type::Unknown`] for any context that imposes no constraint —
/// the caller should treat that as "skip the type check".
pub(crate) fn infer_expr_type_from_context(
    node: NodeRef,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Type {
    crate::inference::expected_type_with_bindings(node, bindings, ctx)
}

pub(crate) fn infer_expr_type_inner(
    node: NodeRef,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Type {
    let file = node.file;
    let Some(tree) = ctx.tree(file) else {
        return Type::Unknown;
    };
    let node = node.value;
    let Some(parent) = tree.parent(node) else {
        return Type::Unknown;
    };

    match tree.kind(parent) {
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
            let this_type = compute_this_type(NodeRef::new(file, parent), bindings, ctx);
            let Some(entry) = nodes::ObjectEntry::cast(tree, parent) else {
                return Type::Unknown;
            };
            if entry.key(tree) == Some(node) {
                container_key_type(&this_type, ctx).unwrap_or(Type::Unknown)
            } else if entry.value(tree) == Some(node) {
                container_value_type(&this_type, ctx).unwrap_or(Type::Unknown)
            } else {
                Type::Unknown
            }
        }

        // `...other` inside an ObjectBody. `other` should be an Object/Listing/
        // Mapping (whichever matches the enclosing this-type).
        NodeKind::ObjectSpread => compute_this_type(NodeRef::new(file, parent), bindings, ctx),

        // `[[predicate]] = value`. Children: [conditionExpr, valueExpr].
        //   - conditionExpr → Boolean
        //   - valueExpr     → element type of enclosing container
        NodeKind::MemberPredicate => {
            let Some(predicate) = nodes::MemberPredicate::cast(tree, parent) else {
                return Type::Unknown;
            };
            if predicate.condition(tree) == Some(node) {
                ctx.base.boolean_type().clone()
            } else if predicate.value(tree) == Some(node) {
                let this_type = compute_this_type(NodeRef::new(file, parent), bindings, ctx);
                container_value_type(&this_type, ctx).unwrap_or(Type::Unknown)
            } else {
                Type::Unknown
            }
        }

        // `when (cond) { body }`. The cond child → Boolean.
        // Body's ObjectBody contents are checked via their own enclosing
        // context (the outer object), not via this arm.
        NodeKind::WhenGenerator => {
            if nodes::WhenGenerator::cast(tree, parent).and_then(|n| n.condition(tree))
                != Some(node)
            {
                Type::Unknown
            } else {
                ctx.base.boolean_type().clone()
            }
        }

        // `for (binding in iterable) { body }`. Iterable expr position:
        //   - Listing<E>, List<E>, Set<E>, Mapping<K,V>, Map<K,V>, or IntRange
        NodeKind::ForGenerator => {
            let iterable = nodes::ForGenerator::cast(tree, parent).and_then(|n| n.iterable(tree));
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
            infer_type_from_arg_list(NodeRef::new(file, node), parent, ctx, bindings)
                .unwrap_or(Type::Unknown)
        }

        // `foo[idx]`. Receiver determines index type:
        //   - Listing<E> / List<E>: Int
        //   - Mapping<K,V> / Map<K,V>: K
        //   - String: Int
        //   - Else: Unknown
        NodeKind::SubscriptExpr => {
            let Some(subscript) = nodes::SubscriptExpr::cast(tree, parent) else {
                return Type::Unknown;
            };
            let Some(receiver) = subscript.receiver(tree) else {
                return Type::Unknown;
            };
            let receiver_type = compute_expr_type(NodeRef::new(file, receiver), bindings, ctx);

            if subscript.index(tree) != Some(node) {
                return Type::Unknown;
            }
            if receiver_type.is_subtype_of(ctx.base.list_type(), ctx.db)
                || receiver_type.is_subtype_of(ctx.base.listing_type(), ctx.db)
                || receiver_type.is_subtype_of(ctx.base.string_type(), ctx.db)
            {
                ctx.base.int_type().clone()
            } else if receiver_type.is_subtype_of(ctx.base.map_type(), ctx.db)
                || receiver_type.is_subtype_of(ctx.base.mapping_type(), ctx.db)
            {
                let Type::Class { type_args, .. } = receiver_type.unconstrained() else {
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
            let Some(other) = other_operand(tree, parent, node) else {
                return Type::Unknown;
            };
            let other_type = compute_expr_type(NodeRef::new(file, other), bindings, ctx);
            if other_type.is_subtype_of(ctx.base.duration_type(), ctx.db) {
                Type::union(
                    [
                        ctx.base.number_type().clone(),
                        ctx.base.duration_type().clone(),
                    ],
                    ctx.db,
                )
            } else if other_type.is_subtype_of(ctx.base.data_size_type(), ctx.db) {
                Type::union(
                    [
                        ctx.base.number_type().clone(),
                        ctx.base.data_size_type().clone(),
                    ],
                    ctx.db,
                )
            } else {
                // TODO: make this more exhaustive (support union types and return a pseudo type alias of multiplicative operands)
                ctx.base.number_type().clone()
            }
        }

        // `a + b`, `a - b` — Number, String, Bytes, List, Set, or Map
        // depending on the *other* operand's type.
        NodeKind::AdditiveExpr => {
            let Some(other) = other_operand(tree, parent, node) else {
                return Type::Unknown;
            };
            compute_expr_type(NodeRef::new(file, other), bindings, ctx)
        }

        // `a < b`, etc. — comparable: Number, Duration, DataSize, String, ...
        NodeKind::ComparisonExpr => ctx.base.comparable_type().clone(),

        // `a && b`, `a || b` — Boolean both sides.
        NodeKind::LogicalAndExpr | NodeKind::LogicalOrExpr => ctx.base.boolean_type().clone(),

        // `a |> b` — pipe. `b` must be a `(typeof(a)) -> R` function.
        //   - If `node` is the left side: Unknown (no constraint on input).
        NodeKind::PipeExpr => {
            let Some(expr) = nodes::BinaryExpr::cast(tree, parent) else {
                return Type::Unknown;
            };
            if expr.right(tree) == Some(node) {
                let Some(left) = expr.left(tree) else {
                    return Type::Unknown;
                };
                let input_type = compute_expr_type(NodeRef::new(file, left), bindings, ctx);
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

        // `-x` — Number, Duration, or DataSize.
        NodeKind::UnaryMinusExpr => Type::union(
            [
                ctx.base.number_type().clone(),
                ctx.base.duration_type().clone(),
                ctx.base.data_size_type().clone(),
            ],
            ctx.db,
        ),

        // `!x` — Boolean.
        NodeKind::LogicalNotExpr => ctx.base.boolean_type().clone(),

        // ===================================================================
        // Control flow / structural
        // ===================================================================

        // `if (c) t else e`. Children: [c, t, e].
        //   - c → Boolean
        //   - t, e → recurse on outer context (infer_expr_type_from_context(NodeRef::new(file, parent), ...))
        NodeKind::IfExpr => {
            if nodes::IfExpr::cast(tree, parent).and_then(|n| n.condition(tree)) == Some(node) {
                ctx.base.boolean_type().clone()
            } else {
                infer_expr_type_from_context(NodeRef::new(file, parent), bindings, ctx)
            }
        }

        // `let (binding = init) body`. Children: [TypedIdentifier, init, body].
        //   - init → the TypedIdentifier's declared type (via to_type_with_bindings)
        //   - body → recurse on outer context
        NodeKind::LetExpr => {
            let Some(expr) = nodes::LetExpr::cast(tree, parent) else {
                return Type::Unknown;
            };
            if expr.value(tree) == Some(node) {
                let ty = expr
                    .binding(tree)
                    .and_then(|n| nodes::TypedIdentifier::cast(tree, n))
                    .and_then(|n| n.type_annotation(tree))
                    .and_then(|n| nodes::TypeAnnotation::cast(tree, n))
                    .and_then(|n| n.ty(tree));
                ty.map(|n| to_type_with_bindings(NodeRef::new(file, n), bindings, ctx))
                    .unwrap_or(Type::Unknown)
            } else if expr.body(tree) == Some(node) {
                infer_expr_type_from_context(NodeRef::new(file, parent), bindings, ctx)
            } else {
                Type::Unknown
            }
        }

        // `(expr)` — fully transparent. Climb to grandparent's context.
        NodeKind::ParenthesizedExpr => {
            infer_expr_type_from_context(NodeRef::new(file, parent), bindings, ctx)
        }

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
        _ => {
            parent_declared_type(NodeRef::new(file, parent), bindings, ctx).unwrap_or(Type::Unknown)
        }
    }
}

/// Return the opposite named operand only when the node is an operand.
fn other_operand(
    tree: &lspkl_syntax::ast::SyntaxTree,
    parent: NodeId,
    node: NodeId,
) -> Option<NodeId> {
    let expr = nodes::BinaryExpr::cast(tree, parent)?;
    if expr.left(tree) == Some(node) {
        expr.right(tree)
    } else if expr.right(tree) == Some(node) {
        expr.left(tree)
    } else {
        None
    }
}

/// Find the type annotation attached to a *member-like* parent
fn parent_declared_type(
    parent: NodeRef,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<Type> {
    let file = parent.file;
    let tree = ctx.tree(file)?;
    let parent = parent.value;
    let annotation = crate::resolver::member_type_annotation(tree, parent)?;
    let type_node = nodes::TypeAnnotation::cast(tree, annotation)?.ty(tree)?;

    Some(to_type_with_bindings(
        NodeRef::new(file, type_node),
        bindings,
        ctx,
    ))
}

fn infer_type_from_arg_list(
    node: NodeRef,
    parent: NodeId,
    ctx: &AnalysisContext,
    bindings: &TypeBindings,
) -> Option<Type> {
    let file = node.file;
    let tree = ctx.tree(file)?;
    let node = node.value;
    let access = tree.parent(parent)?;
    let receiver_type =
        access_receiver_type_with_bindings(NodeRef::new(file, access), bindings, ctx)?;
    let name = tree.name(access).unwrap_or("");
    let method = find_member_on_type(&receiver_type, name, bindings, ctx)?;
    let receiver_type = receiver_type.unaliased(ctx.db);
    let member_bindings;
    let bindings = if matches!(receiver_type.unconstrained(), Type::Class { .. }) {
        member_bindings = crate::resolver::class_member_bindings(&receiver_type, bindings, ctx);
        &member_bindings
    } else {
        bindings
    };
    let arg_index = nodes::ArgumentList::cast(tree, parent)?
        .arguments(tree)
        .position(|c| c == node)?;
    let tree = ctx.tree(method.node.file)?;
    let header = nodes::ClassMethod::cast(tree, method.node.value)
        .and_then(|n| n.header(tree))
        .or_else(|| {
            nodes::ObjectMethod::cast(tree, method.node.value).and_then(|n| n.header(tree))
        })?;
    let param_list = nodes::MethodHeader::cast(tree, header)?.parameters(tree)?;
    let param_decl = nodes::ParameterList::cast(tree, param_list)?
        .parameters(tree)
        .nth(arg_index)?;
    let type_annotation = nodes::TypedIdentifier::cast(tree, param_decl)?.type_annotation(tree)?;
    let type_node = nodes::TypeAnnotation::cast(tree, type_annotation)?.ty(tree)?;
    Some(to_type_with_bindings(
        NodeRef::new(method.node.file, type_node),
        bindings,
        ctx,
    ))
}
