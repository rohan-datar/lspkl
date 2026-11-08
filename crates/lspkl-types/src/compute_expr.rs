//! Type inference for expressions.

use lspkl_syntax::ast::nodes;
use lspkl_syntax::ast::{NodeKind, TokenType};

use crate::NodeRef;
use crate::context::AnalysisContext;
use crate::resolver::{find_member_on_type, resolve_unqualified_access_with_bindings};
use crate::to_type::to_type_with_bindings;
use crate::ty::{Type, TypeBindings};

/// Infer the [`Type`] of an expression node.
///
/// `node` should be an expression — anything `NodeKind::*Expr` plus literals.
/// Returns [`Type::Unknown`] for kinds the inferencer can't yet handle, so
/// callers don't have to special-case missing arms during early development.
pub(crate) fn compute_expr_type(
    node: NodeRef,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Type {
    crate::inference::expression_type(ctx.db, node, ctx.root, bindings)
}

pub(crate) fn compute_expr_inner(
    node: NodeRef,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Type {
    let file = node.file;
    let Some(tree) = ctx.tree(file) else {
        return Type::Unknown;
    };
    let node = node.value;
    match tree.kind(node) {
        // boolean expressions
        NodeKind::TrueLiteralExpr
        | NodeKind::FalseLiteralExpr
        | NodeKind::EqualityExpr
        | NodeKind::ComparisonExpr
        | NodeKind::LogicalAndExpr
        | NodeKind::LogicalOrExpr
        | NodeKind::LogicalNotExpr
        | NodeKind::TypeTestExpr => ctx.base.boolean_type().clone(),

        NodeKind::IntLiteralExpr => ctx.base.int_type().clone(),
        NodeKind::FloatLiteralExpr => ctx.base.float_type().clone(),
        NodeKind::ExponentiationExpr => ctx.base.number_type().clone(),
        NodeKind::NullLiteralExpr => ctx.base.null_type().clone(),
        NodeKind::ThrowExpr => Type::Nothing,
        NodeKind::SingleLineStringLiteral | NodeKind::MultiLineStringLiteral => {
            let interpolated = nodes::SingleLineStringLiteral::cast(tree, node)
                .is_some_and(|n| n.interpolations(tree).next().is_some())
                || nodes::MultiLineStringLiteral::cast(tree, node)
                    .is_some_and(|n| n.interpolations(tree).next().is_some());
            if interpolated {
                ctx.base.string_type().clone()
            } else {
                tree.escaped_string_literal(node)
                    .map(Type::string_literal)
                    .unwrap_or(Type::Unknown)
            }
        }
        NodeKind::ParenthesizedExpr | NodeKind::TraceExpr | NodeKind::UnaryMinusExpr => {
            let operand = nodes::ParenthesizedExpr::cast(tree, node)
                .and_then(|n| n.inner(tree))
                .or_else(|| nodes::TraceExpr::cast(tree, node).and_then(|n| n.value(tree)))
                .or_else(|| nodes::UnaryExpr::cast(tree, node).and_then(|n| n.operand(tree)));
            operand
                .map(|n| compute_expr_type(NodeRef::new(file, n), bindings, ctx))
                .unwrap_or(Type::Unknown)
        }
        NodeKind::LetExpr => nodes::LetExpr::cast(tree, node)
            .and_then(|n| n.body(tree))
            .map(|n| compute_expr_type(NodeRef::new(file, n), bindings, ctx))
            .unwrap_or(Type::Unknown),
        NodeKind::IfExpr => {
            let Some(expr) = nodes::IfExpr::cast(tree, node) else {
                return Type::Unknown;
            };
            let (Some(then_branch), Some(else_branch)) =
                (expr.then_branch(tree), expr.else_branch(tree))
            else {
                return Type::Unknown;
            };
            Type::union(
                [
                    compute_expr_type(NodeRef::new(file, then_branch), bindings, ctx),
                    compute_expr_type(NodeRef::new(file, else_branch), bindings, ctx),
                ],
                ctx.db,
            )
        }
        NodeKind::AdditiveExpr => additive_expr_type(NodeRef::new(file, node), bindings, ctx),
        NodeKind::MultiplicativeExpr => {
            multiplicative_expr_type(NodeRef::new(file, node), bindings, ctx)
        }
        NodeKind::QualifiedAccessExpr => {
            let Some(access) = nodes::QualifiedAccessExpr::cast(tree, node) else {
                return Type::Unknown;
            };
            let Some(reciever) = access.receiver(tree) else {
                return Type::Unknown;
            };
            let Some(name) = tree.name(node) else {
                return Type::Unknown;
            };
            let reciever_type = compute_expr_type(NodeRef::new(file, reciever), bindings, ctx);

            let is_null_safe = access.is_null_safe(tree);
            let member_ty = find_member_on_type(&reciever_type, name, bindings, ctx).map_or(
                Type::Unknown,
                |r| {
                    call_result_type(
                        NodeRef::new(file, node),
                        r,
                        Some(&reciever_type),
                        bindings,
                        ctx,
                    )
                },
            );
            if is_null_safe {
                member_ty.nullable(ctx.db)
            } else {
                member_ty
            }
        }
        NodeKind::UnqualifiedAccessExpr => {
            let Some(name) = tree.name(node) else {
                return Type::Unknown;
            };
            resolve_unqualified_access_with_bindings(NodeRef::new(file, node), name, bindings, ctx)
                .map(|r| {
                    if tree.is_property_access(node) {
                        return guarded_access_type(
                            NodeRef::new(file, node),
                            name,
                            r,
                            bindings,
                            ctx,
                        );
                    }
                    let receiver = crate::resolver::access_receiver_type_with_bindings(
                        NodeRef::new(file, node),
                        bindings,
                        ctx,
                    );
                    call_result_type(
                        NodeRef::new(file, node),
                        r,
                        receiver.as_ref(),
                        bindings,
                        ctx,
                    )
                })
                .unwrap_or(Type::Unknown)
        }
        NodeKind::ImportExpr => crate::resolver::import_type(NodeRef::new(file, node), ctx),
        NodeKind::ThisExpr => compute_this_type(NodeRef::new(file, node), bindings, ctx),
        NodeKind::SuperAccessExpr => crate::resolver::access_receiver_type_with_bindings(
            NodeRef::new(file, node),
            bindings,
            ctx,
        )
        .and_then(|t| {
            tree.name(node)
                .and_then(|n| find_member_on_type(&t, n, bindings, ctx))
                .map(|r| call_result_type(NodeRef::new(file, node), r, Some(&t), bindings, ctx))
        })
        .unwrap_or(Type::Unknown),
        NodeKind::SubscriptExpr => nodes::SubscriptExpr::cast(tree, node)
            .and_then(|n| n.receiver(tree))
            .map(|e| compute_expr_type(NodeRef::new(file, e), bindings, ctx))
            .and_then(|t| crate::resolver::container_value_type(&t, ctx))
            .unwrap_or(Type::Unknown),
        NodeKind::ModuleExpr => Type::Module {
            module_id: crate::module_id(ctx.db, file, ctx.root),
        },
        NodeKind::NewExpr => nodes::NewExpr::cast(tree, node)
            .and_then(|n| n.ty(tree))
            .map(|tn| to_type_with_bindings(NodeRef::new(file, tn), bindings, ctx))
            .unwrap_or_else(|| {
                crate::infer_expr_type::infer_expr_type_from_context(
                    NodeRef::new(file, node),
                    bindings,
                    ctx,
                )
                .non_null(ctx.db)
            }),
        NodeKind::AmendExpr => nodes::AmendExpr::cast(tree, node)
            .and_then(|n| n.parent(tree))
            .map(|parent| compute_expr_type(NodeRef::new(file, parent), bindings, ctx))
            .unwrap_or(Type::Unknown),
        _ => Type::Unknown,
    }
}

/// A tested receiver can be used at its tested type in the true branch.
/// Compare declaration identities as well as names so a nested binding cannot
/// accidentally inherit a guard on an outer variable. Body boundaries reset
/// the receiver/lexical environment and are deliberately not crossed.
fn guarded_access_type(
    access: NodeRef,
    name: &str,
    resolved: crate::resolver::ResolvedType,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Type {
    let Some(tree) = ctx.tree(access.file) else {
        return resolved.ty;
    };
    let mut child = access.value;
    while let Some(parent) = tree.parent(child) {
        if matches!(
            tree.kind(parent),
            NodeKind::ObjectBody | NodeKind::FunctionLiteralExpr
        ) {
            break;
        }
        if let Some(branch) = nodes::IfExpr::cast(tree, parent)
            && branch.then_branch(tree) == Some(child)
            && let Some(condition) = branch.condition(tree)
            && matches!(tree.kind(condition), NodeKind::TypeTestExpr)
            && let Some(test) = nodes::BinaryExpr::cast(tree, condition)
            && let (Some(operand), Some(target)) = (test.left(tree), test.right(tree))
            && matches!(tree.kind(operand), NodeKind::UnqualifiedAccessExpr)
            && tree.name(operand) == Some(name)
            && tree.is_property_access(operand)
            && resolve_unqualified_access_with_bindings(
                access.with_value(operand),
                name,
                bindings,
                ctx,
            )
            .is_some_and(|tested| tested.node == resolved.node)
        {
            return to_type_with_bindings(access.with_value(target), bindings, ctx);
        }
        child = parent;
    }
    resolved.ty
}

/// Bind a method's explicit type arguments in the caller's lexical context.
/// Receiver substitutions are retained for parameters inherited from classes.
pub(crate) fn call_bindings(
    access: NodeRef,
    method: NodeRef,
    receiver: Option<&Type>,
    caller_bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> TypeBindings {
    let mut bindings = receiver.map_or_else(
        || caller_bindings.clone(),
        |receiver| {
            crate::resolver::class_member_bindings(
                &receiver.unaliased(ctx.db),
                caller_bindings,
                ctx,
            )
        },
    );
    let Some(decl) = crate::decl_for_node(ctx.db, method, ctx.root) else {
        return bindings;
    };
    if let Some(receiver) = receiver {
        crate::type_resolution::bind_this_type(&mut bindings, decl, receiver);
    }
    let Some(tree) = ctx.tree(access.file) else {
        return bindings;
    };
    let arguments = nodes::UnqualifiedAccessExpr::cast(tree, access.value)
        .and_then(|n| n.type_arguments(tree))
        .or_else(|| {
            nodes::QualifiedAccessExpr::cast(tree, access.value)
                .and_then(|n| n.type_arguments(tree))
        })
        .or_else(|| {
            nodes::SuperAccessExpr::cast(tree, access.value).and_then(|n| n.type_arguments(tree))
        })
        .and_then(|n| nodes::TypeArgumentList::cast(tree, n));
    let Some(arguments) = arguments else {
        return bindings;
    };
    let arguments: Vec<_> = arguments.arguments(tree).collect();
    let parameters = crate::type_resolution::parameters(ctx.db, decl);
    // Incomplete/wrong-arity calls stay permissive instead of partially binding.
    for (index, _) in parameters.iter().enumerate() {
        let ty = if arguments.len() == parameters.len() {
            to_type_with_bindings(access.with_value(arguments[index]), caller_bindings, ctx)
        } else {
            Type::Unknown
        };
        bindings.insert(
            crate::TypeParameterId {
                declaration: decl,
                index: index as u32,
            },
            ty,
        );
    }
    bindings
}

fn call_result_type(
    access: NodeRef,
    resolved: crate::resolver::ResolvedType,
    receiver: Option<&Type>,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Type {
    let Some(tree) = ctx.tree(access.file) else {
        return Type::Unknown;
    };
    if tree.is_property_access(access.value) {
        return resolved.ty;
    }
    let bindings = call_bindings(access, resolved.node, receiver, bindings, ctx);
    crate::decl_for_node(ctx.db, resolved.node, ctx.root)
        .map(|decl| crate::resolver::declaration_type_with_bindings(ctx.db, decl, &bindings))
        .unwrap_or(resolved.ty)
}

/// Compute the type of `this` at a given AST position.
pub(crate) fn compute_this_type(
    node: NodeRef,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Type {
    crate::inference::this_type_with_bindings(node, bindings, ctx)
}

pub(crate) fn compute_this_type_inner(
    node: NodeRef,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Type {
    let file = node.file;
    let Some(tree) = ctx.tree(file) else {
        return Type::Unknown;
    };
    let node = node.value;
    let mut current = Some(node);
    let mut previous = None;
    while let Some(id) = current {
        match tree.kind(id) {
            // `[[condition]]` evaluates its condition with `this` bound to each
            // member's value (a custom-this scope), not the enclosing object.
            NodeKind::MemberPredicate
                if previous.is_some()
                    && nodes::MemberPredicate::cast(tree, id)
                        .and_then(|predicate| predicate.condition(tree))
                        == previous =>
            {
                let receiver = compute_this_type(NodeRef::new(file, id), bindings, ctx);
                return crate::resolver::container_value_type(&receiver, ctx)
                    .unwrap_or(Type::Unknown);
            }
            NodeKind::Module => {
                return Type::Module {
                    module_id: crate::module_id(ctx.db, file, ctx.root),
                };
            }
            NodeKind::Class => {
                return crate::class_id(ctx.db, NodeRef::new(file, id), ctx.root)
                    .map(Type::class)
                    .unwrap_or(Type::Unknown);
            }
            NodeKind::ConstrainedType => {
                let Some(inner) =
                    nodes::ConstrainedType::cast(tree, id).and_then(|n| n.inner(tree))
                else {
                    return Type::Unknown;
                };
                return to_type_with_bindings(NodeRef::new(file, inner), bindings, ctx);
            }
            NodeKind::TypeAlias => return Type::Unknown,
            NodeKind::NewExpr => {
                return compute_expr_type(NodeRef::new(file, id), bindings, ctx);
            }
            NodeKind::AmendExpr => {
                // The receiver expression is evaluated in the *outer* scope.
                // Only the amendment body introduces the amended receiver.
                if id == node {
                    return compute_expr_type(NodeRef::new(file, id), bindings, ctx);
                }
                current = tree.parent(id);
            }
            NodeKind::ObjectBody => {
                let Some(parent) = tree.parent(id) else {
                    return Type::Unknown;
                };
                let owner = NodeRef::new(file, parent);
                match tree.kind(parent) {
                    NodeKind::NewExpr | NodeKind::AmendExpr => {
                        return compute_expr_type(owner, bindings, ctx);
                    }
                    NodeKind::ClassProperty | NodeKind::ObjectProperty => {
                        return crate::infer_expr_type::property_body_type(owner, bindings, ctx)
                            .non_null(ctx.db);
                    }
                    NodeKind::ObjectEntry | NodeKind::MemberPredicate => {
                        let receiver = compute_this_type(owner, bindings, ctx);
                        return crate::resolver::container_value_type(&receiver, ctx)
                            .unwrap_or(Type::Unknown);
                    }
                    NodeKind::Annotation => {
                        let Some(name) = nodes::Annotation::cast(tree, parent)
                            .and_then(|annotation| annotation.name(tree))
                        else {
                            return Type::Unknown;
                        };
                        return crate::resolver::resolve_type_name(
                            ctx,
                            &tree.qualified_identifier_segments(name),
                            NodeRef::new(file, name),
                        )
                        .and_then(|node| crate::class_id(ctx.db, node, ctx.root))
                        .map(Type::class)
                        .unwrap_or(Type::Unknown);
                    }
                    // Generator bodies do not introduce a new receiver.
                    NodeKind::WhenGenerator | NodeKind::ForGenerator => current = Some(parent),
                    _ => return Type::Unknown,
                }
            }
            _ => current = tree.parent(id),
        }
        previous = Some(id);
    }
    Type::Unknown
}

fn additive_expr_type(node: NodeRef, bindings: &TypeBindings, ctx: &AnalysisContext) -> Type {
    let file = node.file;
    let node = node.value;
    use NumKind::*;
    let Some((left, op, right)) = binary_expr(NodeRef::new(file, node), bindings, ctx) else {
        return Type::Unknown;
    };
    let l = classify(&left, ctx);
    let r = classify(&right, ctx);

    match (l, op, r) {
        (Unknown, _, _) | (_, _, Unknown) => Type::Unknown,

        // Numeric arithmetic — both `+` and `-` are valid.
        (Int, _, Int) => ctx.base.int_type().clone(),
        (Int, _, Boolean) => ctx.base.boolean_type().clone(),
        (Float, _, Int | Float) => ctx.base.float_type().clone(),

        // Concatenation forms — only `+`.
        (String | StringLiteral, _, String | StringLiteral) if op == TokenType::Plus => {
            ctx.base.string_type().clone()
        }
        (Bytes, _, Bytes) if op == TokenType::Plus => ctx.base.bytes_type().clone(),

        (List, _, List | Set) if op == TokenType::Plus => {
            union_single_arg(&left, &right, ctx.base.list_type(), ctx)
        }
        (Set, _, Set) if op == TokenType::Plus => {
            union_single_arg(&left, &right, ctx.base.set_type(), ctx)
        }
        (Map, _, Map) if op == TokenType::Plus => {
            union_two_args(&left, &right, ctx.base.map_type(), ctx)
        }

        _ => Type::Unknown,
    }
}

fn multiplicative_expr_type(node: NodeRef, bindings: &TypeBindings, ctx: &AnalysisContext) -> Type {
    let file = node.file;
    let node = node.value;
    use NumKind::*;
    let Some((left, op, right)) = binary_expr(NodeRef::new(file, node), bindings, ctx) else {
        return Type::Unknown;
    };
    let l = classify(&left, ctx);
    let r = classify(&right, ctx);

    match (l, op, r) {
        (Int, TokenType::Star, Int) => ctx.base.int_type().clone(),
        (Float, TokenType::Star, _) | (_, TokenType::Star, Float) => ctx.base.float_type().clone(),

        (Int | Float, TokenType::Slash, _) | (_, TokenType::Slash, Int | Float) => {
            ctx.base.float_type().clone()
        }
        (Int | Float, TokenType::IntDiv, _) | (_, TokenType::IntDiv, Int | Float) => {
            ctx.base.int_type().clone()
        }

        (Int, TokenType::Mod, Int) => ctx.base.int_type().clone(),
        (Float, TokenType::Mod, _) | (_, TokenType::Mod, Float) => ctx.base.float_type().clone(),
        _ => Type::Unknown,
    }
}

/// Local classification of a [`Type`] for binary-arithmetic dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NumKind {
    Unknown,
    Int,
    Float,
    Boolean,
    String,
    StringLiteral,
    Bytes,
    List,
    Set,
    Map,
    Other,
}

fn classify(ty: &Type, ctx: &AnalysisContext) -> NumKind {
    let base = ctx.base;
    match ty.unconstrained() {
        Type::Unknown => NumKind::Unknown,
        Type::StringLiteral { .. } => NumKind::StringLiteral,
        _ if ty.class_equals(base.int_type()) => NumKind::Int,
        _ if ty.class_equals(base.float_type()) => NumKind::Float,
        _ if ty.class_equals(base.boolean_type()) => NumKind::Boolean,
        _ if ty.class_equals(base.string_type()) => NumKind::String,
        _ if ty.class_equals(base.bytes_type()) => NumKind::Bytes,
        _ if ty.class_equals(base.list_type()) => NumKind::List,
        _ if ty.class_equals(base.set_type()) => NumKind::Set,
        _ if ty.class_equals(base.map_type()) => NumKind::Map,
        _ => NumKind::Other,
    }
}

/// Build `target<L|R>` where `L` is the first type-arg of `left` and `R` of
/// `right`. Used for List/Set concat. Returns `Type::Unknown` if either side
/// isn't a `Type::Class`
fn union_single_arg(left: &Type, right: &Type, target: &Type, ctx: &AnalysisContext) -> Type {
    let (Some(l_arg), Some(r_arg)) = (type_arg(left, 0), type_arg(right, 0)) else {
        return Type::Unknown;
    };
    let unioned = Type::union([l_arg.clone(), r_arg.clone()], ctx.db);
    target.clone().with_type_args(vec![unioned])
}

/// Build `target<Lk|Rk, Lv|Rv>` for two-arg generics (Map).
fn union_two_args(left: &Type, right: &Type, target: &Type, ctx: &AnalysisContext) -> Type {
    let (Some(lk), Some(rk), Some(lv), Some(rv)) = (
        type_arg(left, 0),
        type_arg(right, 0),
        type_arg(left, 1),
        type_arg(right, 1),
    ) else {
        return Type::Unknown;
    };
    let key = Type::union([lk.clone(), rk.clone()], ctx.db);
    let value = Type::union([lv.clone(), rv.clone()], ctx.db);
    target.clone().with_type_args(vec![key, value])
}

fn type_arg(ty: &Type, idx: usize) -> Option<&Type> {
    match ty.unconstrained() {
        Type::Class { type_args, .. } => type_args.get(idx),
        _ => None,
    }
}

fn binary_expr(
    node: NodeRef,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<(Type, TokenType, Type)> {
    let file = node.file;
    let tree = ctx.tree(file)?;
    let node = node.value;
    let expr = nodes::BinaryExpr::cast(tree, node)?;
    Some((
        compute_expr_type(NodeRef::new(file, expr.left(tree)?), bindings, ctx),
        expr.operator(tree)?,
        compute_expr_type(NodeRef::new(file, expr.right(tree)?), bindings, ctx),
    ))
}
