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

        // Plain Listing elements have the same value expectation as entries.
        NodeKind::ObjectElement => {
            let Some(body) = tree.parent(parent) else {
                return Type::Unknown;
            };
            let Some(object) = tree.parent(body) else {
                return Type::Unknown;
            };
            let Some(new) = nodes::NewExpr::cast(tree, object) else {
                return Type::Unknown;
            };
            if new.body(tree) != Some(body) {
                return Type::Unknown;
            }
            let container = if let Some(ty) = new.ty(tree) {
                to_type_with_bindings(NodeRef::new(file, ty), bindings, ctx)
            } else {
                infer_expr_type_from_context(NodeRef::new(file, object), bindings, ctx)
            };
            container_value_type(&container, ctx).unwrap_or(Type::Unknown)
        }

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
        NodeKind::ObjectSpread => {
            let receiver = compute_this_type(NodeRef::new(file, parent), bindings, ctx);
            let Some(element) = container_value_type(&receiver, ctx) else {
                // Dynamic and unavailable receiver shapes stay open.
                return Type::Unknown;
            };
            let mut sources = vec![receiver.clone(), ctx.base.dynamic_type().clone()];
            if receiver.is_subtype_of(ctx.base.listing_type(), ctx.db) {
                sources.push(
                    ctx.base
                        .collection_type()
                        .clone()
                        .with_type_args(vec![element.clone()]),
                );
                if ctx.base.int_type().is_subtype_of(&element, ctx.db) {
                    sources.push(ctx.base.bytes_type().clone());
                }
            } else if receiver.is_subtype_of(ctx.base.mapping_type(), ctx.db) {
                let key = container_key_type(&receiver, ctx).unwrap_or(Type::Unknown);
                sources.push(
                    ctx.base
                        .map_type()
                        .clone()
                        .with_type_args(vec![key, element]),
                );
            }
            let expected = Type::union(sources, ctx.db);
            if nodes::ObjectSpread::cast(tree, parent)
                .is_some_and(|spread| spread.is_null_safe(tree))
            {
                expected.nullable(ctx.db)
            } else {
                expected
            }
        }
        NodeKind::ObjectElement => {
            let receiver = compute_this_type(NodeRef::new(file, parent), bindings, ctx);
            container_value_type(&receiver, ctx).unwrap_or(Type::Unknown)
        }

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
        //   - Collection (List, Set), Map, IntSeq, Bytes, or an untyped object
        //     container (Listing, Mapping, Dynamic)
        NodeKind::ForGenerator => {
            let iterable = nodes::ForGenerator::cast(tree, parent).and_then(|n| n.iterable(tree));
            if Some(node) == iterable {
                let base = ctx.base;
                Type::union(
                    [
                        base.collection_type().clone(),
                        base.map_type().clone(),
                        base.int_seq_type().clone(),
                        base.bytes_type().clone(),
                        base.listing_type().clone(),
                        base.mapping_type().clone(),
                        base.dynamic_type().clone(),
                    ],
                    ctx.db,
                )
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
            let other_type = compute_expr_type(NodeRef::new(file, other), bindings, ctx);
            additive_operand_type(&other_type, ctx)
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
    let Some(annotation) = crate::resolver::member_type_annotation(tree, parent) else {
        return matches!(
            tree.kind(parent),
            NodeKind::ClassProperty | NodeKind::ObjectProperty
        )
        .then(|| inherited_property_type(NodeRef::new(file, parent), bindings, ctx));
    };
    let type_node = nodes::TypeAnnotation::cast(tree, annotation)?.ty(tree)?;

    Some(to_type_with_bindings(
        NodeRef::new(file, type_node),
        bindings,
        ctx,
    ))
}

/// A property amendment uses its annotation, value, or inherited declaration,
/// never inference of the body itself.
pub(crate) fn property_body_type(
    property: NodeRef,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Type {
    let Some(tree) = ctx.tree(property.file) else {
        return Type::Unknown;
    };
    if crate::resolver::member_type_annotation(tree, property.value).is_some() {
        return parent_declared_type(property, bindings, ctx).unwrap_or(Type::Unknown);
    }
    let value = nodes::ClassProperty::cast(tree, property.value)
        .and_then(|p| p.value(tree))
        .or_else(|| nodes::ObjectProperty::cast(tree, property.value).and_then(|p| p.value(tree)));
    if let Some(value) = value {
        return compute_expr_type(property.with_value(value), bindings, ctx);
    }
    inherited_property_type(property, bindings, ctx)
}

pub(crate) fn inherited_property_type(
    property: NodeRef,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Type {
    let Some(tree) = ctx.tree(property.file) else {
        return Type::Unknown;
    };
    let Some(name) = tree.name(property.value) else {
        return Type::Unknown;
    };
    let receiver = compute_this_type(property, bindings, ctx);
    // A lowered override shadows the inherited declaration; do not infer its
    // own initializer to discover that initializer's contextual type.
    let receiver = if crate::decl_for_node(ctx.db, property, ctx.root).is_some() {
        let parent = crate::resolver::super_type(&receiver, ctx.db).or_else(|| {
            matches!(receiver.unconstrained(), Type::Module { .. })
                .then(|| ctx.base.module_type().clone())
        });
        let Some(parent) = parent else {
            return Type::Unknown;
        };
        parent
    } else {
        receiver
    };
    find_member_on_type(&receiver, name, bindings, ctx).map_or(Type::Unknown, |member| member.ty)
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
    let access_ref = NodeRef::new(file, access);
    let name = tree.name(access).unwrap_or("");
    let receiver_type = access_receiver_type_with_bindings(access_ref, bindings, ctx);
    let on_receiver = receiver_type
        .as_ref()
        .and_then(|ty| find_member_on_type(ty, name, bindings, ctx));
    // Unqualified calls resolve lexically: a local or module function called
    // inside an object body is not a member of that object's `this` type
    // (which may also be unavailable there). Receiver substitutions apply only
    // when the lexical target is that receiver's member.
    let (method, receiver_type) = if matches!(tree.kind(access), NodeKind::UnqualifiedAccessExpr) {
        let lexical = crate::resolver::resolve_unqualified_access_with_bindings(
            access_ref, name, bindings, ctx,
        )?;
        let receiver = receiver_type.filter(|_| {
            on_receiver
                .as_ref()
                .is_some_and(|member| member.node == lexical.node)
        });
        (lexical, receiver)
    } else {
        (on_receiver?, receiver_type)
    };
    // Receiver and explicit call-site type arguments bind the callee's parameters.
    let member_bindings = crate::compute_expr::call_bindings(
        access_ref,
        method.node,
        receiver_type.as_ref(),
        bindings,
        ctx,
    );
    let bindings = &member_bindings;
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
    let parameters: Vec<_> = nodes::ParameterList::cast(tree, param_list)?
        .parameters(tree)
        .collect();
    let param_type = |param_decl| {
        let type_annotation =
            nodes::TypedIdentifier::cast(tree, param_decl)?.type_annotation(tree)?;
        let type_node = nodes::TypeAnnotation::cast(tree, type_annotation)?.ty(tree)?;
        Some(to_type_with_bindings(
            NodeRef::new(method.node.file, type_node),
            bindings,
            ctx,
        ))
    };
    // A trailing `VarArgs<E>` parameter absorbs this and every later
    // argument, each of which must be an `E`.
    let last = parameters.len().checked_sub(1)?;
    let varargs =
        param_type(parameters[last]).filter(|ty| ty.class_equals(ctx.base.varargs_type()));
    if arg_index >= last
        && let Some(varargs) = varargs
    {
        return Some(match varargs.unconstrained() {
            Type::Class { type_args, .. } => type_args.first().cloned().unwrap_or(Type::Unknown),
            _ => Type::Unknown,
        });
    }
    param_type(*parameters.get(arg_index)?)
}

/// The type an additive operand may have, given the other operand's type.
///
/// This is the operand *category* the operator accepts, never the other
/// operand's exact type: `header + text` with a string-literal `header` takes
/// any `String`, and `1 + 2.5` mixes `Int` and `Float`. Collections and maps
/// expect their raw class so differing element types still concatenate.
/// Returns `Unknown` (no constraint) when the other operand is not a known
/// additive type.
fn additive_operand_type(other: &Type, ctx: &AnalysisContext) -> Type {
    let base = ctx.base;
    if matches!(other, Type::Unknown) {
        return Type::Unknown;
    }
    let is = |target: &Type| other.is_subtype_of(target, ctx.db);
    if is(base.number_type()) {
        base.number_type().clone()
    } else if is(base.string_type()) {
        base.string_type().clone()
    } else if is(base.duration_type()) {
        base.duration_type().clone()
    } else if is(base.data_size_type()) {
        base.data_size_type().clone()
    } else if is(base.bytes_type()) {
        base.bytes_type().clone()
    } else if is(base.collection_type()) {
        base.collection_type().clone()
    } else if is(base.map_type()) {
        base.map_type().clone()
    } else {
        Type::Unknown
    }
}
