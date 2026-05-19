//! Type inference for expressions.

use pkl_analysis::ast::{NodeId, NodeKind, TokenType};

use crate::context::AnalysisContext;
use crate::resolver::{find_member_on_type, resolve_local_binding, resolve_type_name_in_module};
use crate::to_type::to_type;
use crate::ty::{Type, TypeBindings};
use crate::{FileId, NodeRef};

/// Infer the [`Type`] of an expression node.
///
/// `node` should be an expression — anything `NodeKind::*Expr` plus literals.
/// Returns [`Type::Unknown`] for kinds the inferencer can't yet handle, so
/// callers don't have to special-case missing arms during early development.
pub fn compute_expr_type(node: NodeId, bindings: &TypeBindings, ctx: &AnalysisContext) -> Type {
    match ctx.tree.kind(node) {
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
            if ctx
                .tree
                .child(node, |n| matches!(n, NodeKind::StringInterpolation))
                .is_some()
            {
                ctx.base.string_type().clone()
            } else {
                let escaped = ctx
                    .tree
                    .escaped_string_literal(node)
                    .expect("unable to escape string literal");
                Type::string_literal(escaped)
            }
        }
        NodeKind::ParenthesizedExpr | NodeKind::TraceExpr => {
            compute_expr_type(child_exprs(ctx, node)[0], bindings, ctx)
        }
        NodeKind::LetExpr => {
            compute_expr_type(*child_exprs(ctx, node).last().unwrap(), bindings, ctx)
        }
        NodeKind::IfExpr => {
            let expr_children = child_exprs(ctx, node);
            Type::Union {
                left: Box::new(compute_expr_type(expr_children[0], bindings, ctx)),
                right: Box::new(compute_expr_type(expr_children[1], bindings, ctx)),
                constraints: vec![],
            }
        }
        NodeKind::UnaryMinusExpr => compute_expr_type(
            ctx.tree
                .child(node, |n| !matches!(n, NodeKind::Terminal(TokenType::Minus)))
                .expect("Unary minus did not have inner expression"),
            bindings,
            ctx,
        ),
        NodeKind::AdditiveExpr => additive_expr_type(node, bindings, ctx),
        NodeKind::MultiplicativeExpr => multiplicative_expr_type(node, bindings, ctx),
        NodeKind::QualifiedAccessExpr => {
            let Some(reciever) = ctx.tree.child(node, |k| k.is_expr_node()) else {
                return Type::Unknown;
            };
            let Some(accessor) = ctx.tree.child(node, |k| {
                matches!(k, NodeKind::Terminal(TokenType::Identifier))
            }) else {
                return Type::Unknown;
            };
            let Some(name) = ctx.tree.name(accessor) else {
                return Type::Unknown;
            };
            let reciever_type = compute_expr_type(reciever, bindings, ctx);
            find_member_on_type(&reciever_type, name, bindings, ctx)
                .map_or(Type::Unknown, |(_, ty)| ty)
        }
        NodeKind::UnqualifiedAccessExpr => {
            let Some(name) = ctx.tree.name(node) else {
                return Type::Unknown;
            };
            if let Some(local_type) = resolve_local_binding(node, name, bindings, ctx) {
                return local_type;
            };
            if let Some(this_type) =
                find_member_on_type(&compute_this_type(node, bindings, ctx), name, bindings, ctx)
                    .map(|(_, ty)| ty)
            {
                return this_type;
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
            )
            .map(|(_, ty)| ty)
            {
                return module_type;
            };
            if let Some(base_type) = resolve_type_name_in_module(ctx.base.tree(), name) {
                return to_type(NodeRef::new(FileId::BASE, base_type), bindings, ctx);
            };

            Type::Unknown
        }
        _ => Type::Unknown,
    }
}

/// Compute the type of `this` at a given AST position.
pub fn compute_this_type(node: NodeId, bindings: &TypeBindings, ctx: &AnalysisContext) -> Type {
    let _ = bindings;
    let mut current = Some(node);
    while let Some(id) = current {
        match ctx.tree.kind(id) {
            NodeKind::Module => {
                return Type::Module {
                    module_node: NodeRef::new(ctx.file_id, id),
                    reference_name: ctx.tree.name(id).unwrap_or("").to_string(),
                    constraints: vec![],
                };
            }
            NodeKind::Class => return Type::class(NodeRef::new(ctx.file_id, id)),
            NodeKind::TypeAlias => return Type::Unknown,
            NodeKind::AmendExpr | NodeKind::NewExpr | NodeKind::ObjectBody => return Type::Unknown, // TODO properly handle these cases
            _ => current = ctx.tree.parent(id),
        }
    }
    Type::Unknown
}

// find non-terminal children of an expression
fn child_exprs(ctx: &AnalysisContext, node: NodeId) -> Vec<NodeId> {
    ctx.tree
        .children_matching(node, |n| !matches!(n, NodeKind::Terminal(_)))
}

fn additive_expr_type(node: NodeId, bindings: &TypeBindings, ctx: &AnalysisContext) -> Type {
    use NumKind::*;
    let (left, op, right) = binary_expr(node, bindings, ctx);
    let l = classify(&left, ctx);
    let r = classify(&right, ctx);

    match (l, op, r) {
        (Unknown, _, _) | (_, _, Unknown) => Type::Unknown,

        // Numeric arithmetic — both `+` and `-` are valid.
        (Int, _, Int) => ctx.base.int_type().clone(),
        (Int, _, Boolean) => ctx.base.boolean_type().clone(), // matches pkl-lsp
        (Float, _, Int | Float) => ctx.base.float_type().clone(),

        // Concatenation forms — only `+`.
        (String | StringLiteral, _, String | StringLiteral) if op == TokenType::Plus => {
            ctx.base.string_type().clone()
        }
        (Bytes, _, Bytes) if op == TokenType::Plus => ctx.base.bytes_type().clone(),

        (List, _, List | Set) if op == TokenType::Plus => {
            union_single_arg(&left, &right, ctx.base.list_type())
        }
        (Set, _, Set) if op == TokenType::Plus => {
            union_single_arg(&left, &right, ctx.base.set_type())
        }
        (Map, _, Map) if op == TokenType::Plus => {
            union_two_args(&left, &right, ctx.base.map_type())
        }

        _ => Type::Unknown,
    }
}

fn multiplicative_expr_type(node: NodeId, bindings: &TypeBindings, ctx: &AnalysisContext) -> Type {
    use NumKind::*;
    let (left, op, right) = binary_expr(node, bindings, ctx);
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
    match ty {
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
fn union_single_arg(left: &Type, right: &Type, target: &Type) -> Type {
    let (Some(l_arg), Some(r_arg)) = (type_arg(left, 0), type_arg(right, 0)) else {
        return Type::Unknown;
    };
    let unioned = Type::Union {
        left: Box::new(l_arg.clone()),
        right: Box::new(r_arg.clone()),
        constraints: vec![],
    };
    with_type_args(target.clone(), vec![unioned])
}

/// Build `target<Lk|Rk, Lv|Rv>` for two-arg generics (Map).
fn union_two_args(left: &Type, right: &Type, target: &Type) -> Type {
    let (Some(lk), Some(rk), Some(lv), Some(rv)) = (
        type_arg(left, 0),
        type_arg(right, 0),
        type_arg(left, 1),
        type_arg(right, 1),
    ) else {
        return Type::Unknown;
    };
    let key = Type::Union {
        left: Box::new(lk.clone()),
        right: Box::new(rk.clone()),
        constraints: vec![],
    };
    let value = Type::Union {
        left: Box::new(lv.clone()),
        right: Box::new(rv.clone()),
        constraints: vec![],
    };
    with_type_args(target.clone(), vec![key, value])
}

fn type_arg(ty: &Type, idx: usize) -> Option<&Type> {
    match ty {
        Type::Class { type_args, .. } => type_args.get(idx),
        _ => None,
    }
}

fn with_type_args(mut ty: Type, args: Vec<Type>) -> Type {
    if let Type::Class {
        ref mut type_args, ..
    } = ty
    {
        *type_args = args;
        ty
    } else {
        Type::Unknown
    }
}

fn binary_expr(
    node: NodeId,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> (Type, TokenType, Type) {
    // break down an expression of the form expr OP expr
    let expr_children = child_exprs(ctx, node);
    if expr_children.len() != 2 {
        panic!("Not a binary expression");
    }
    let op_node = ctx
        .tree
        .child(node, |n| matches!(n, NodeKind::Terminal(_)))
        .expect("No operator in binary expression!");

    let NodeKind::Terminal(op) = ctx.tree.kind(op_node) else {
        panic!("invalid operator in binary expression")
    };
    (
        compute_expr_type(expr_children[0], bindings, ctx),
        *op,
        compute_expr_type(expr_children[1], bindings, ctx),
    )
}
