//! Validates property/method access expressions — unresolved references,
//! const-context violations, recursive property references, argument-count
//! mismatches.

use pkl_types::NodeRef;

use crate::analyzer::Analyzer;
use crate::effective_pkl_version;
use crate::holder::DiagnosticsHolder;
use pkl_syntax::ast::{NodeId, NodeKind, TokenType, nodes};

use pkl_types::find_member;
use pkl_types::resolver::{ResolvedType, resolve_type_name, resolve_unqualified_access};
use pkl_types::{AnalysisContext, Type};
use semver::Version;

pub struct AccessExprAnalyzer;

impl Analyzer for AccessExprAnalyzer {
    fn analyze_node(
        &self,
        node: NodeRef,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        let file = node.file;
        let Some(source_tree) = ctx.tree(file) else {
            return false;
        };
        let node = node.value;
        match source_tree.kind(node) {
            NodeKind::QualifiedAccessExpr => {
                let Some(receiver) = nodes::QualifiedAccessExpr::cast(source_tree, node)
                    .and_then(|expr| expr.receiver(source_tree))
                else {
                    return true;
                };
                let receiver_type = NodeRef::new(file, receiver)
                    .cast(ctx, pkl_syntax::ast::nodes::Expr::cast)
                    .map_or(Type::Unknown, |expr| pkl_types::expr_type(expr, ctx));

                // Suppression: receiver type unknown, or Dynamic on property access
                if matches!(receiver_type, Type::Unknown) {
                    return true;
                }
                if receiver_type.class_equals(ctx.base.dynamic_type())
                    && source_tree.is_property_access(node)
                {
                    return true;
                }

                let name = source_tree.name(node).unwrap_or("");
                let Some(r) = find_member(&receiver_type, name, ctx) else {
                    // Identifier-only span if you can; else the whole expr span
                    let ident = source_tree
                        .identifier(node)
                        .map(|i| source_tree.span(i))
                        .unwrap_or_else(|| source_tree.span(node));
                    holder.add_error(ident, format!("unresolved reference: {name}"));
                    return true;
                };
                if matches!(source_tree.kind(receiver), NodeKind::ThisExpr) {
                    check_recursive(node, r.node, holder, file, ctx);
                }

                if !source_tree.is_property_access(node) {
                    check_argument_count(node, r.node, holder, file, ctx);
                }
                check_const_access(node, r.node, holder, file, ctx);
                check_static_body_access(node, r.node, holder, file, ctx);
                check_redundant_conversion(receiver, &receiver_type, r.node, holder, file, ctx);
            }
            NodeKind::UnqualifiedAccessExpr => {
                // Skip when inside a QualifiedAccessExpr — the receiver was already
                // computed by its parent.
                if let Some(parent) = source_tree.parent(node)
                    && matches!(source_tree.kind(parent), NodeKind::QualifiedAccessExpr)
                    && nodes::QualifiedAccessExpr::cast(source_tree, parent)
                        .and_then(|expr| expr.receiver(source_tree))
                        == Some(node)
                {
                    return true;
                }

                let name = source_tree.name(node).unwrap_or("");
                let Some(r) = resolve_unqualified_access(NodeRef::new(file, node), name, ctx)
                else {
                    // Suppression on this-type:
                    let this_ty = pkl_types::this_type(NodeRef::new(file, node), ctx);
                    if matches!(this_ty, Type::Unknown) {
                        return true;
                    }
                    if this_ty.class_equals(ctx.base.dynamic_type())
                        && source_tree.is_property_access(node)
                    {
                        return true;
                    }
                    // Otherwise: unresolved
                    let ident = source_tree
                        .identifier(node)
                        .map(|i| source_tree.span(i))
                        .unwrap_or_else(|| source_tree.span(node));
                    holder.add_error(ident, format!("unresolved reference: {name}"));
                    return true;
                };
                check_recursive(node, r.node, holder, file, ctx);

                if !source_tree.is_property_access(node) {
                    check_argument_count(node, r.node, holder, file, ctx);
                }
                check_const_access(node, r.node, holder, file, ctx);
                check_static_body_access(node, r.node, holder, file, ctx);
            }
            NodeKind::ModuleExpr | NodeKind::ThisExpr => {
                if let Some(parent) = source_tree.parent(node)
                    && matches!(source_tree.kind(parent), NodeKind::QualifiedAccessExpr)
                {
                    return true;
                }
                let (is_const, is_in_const_scope) = const_scope(node, file, ctx);
                if is_const && !is_in_const_scope {
                    holder.add_error(
                        source_tree.span(node),
                        format!("cannot reference {} from const", source_tree.text(node)),
                    );
                }
            }
            NodeKind::SuperAccessExpr => {
                // Skip if the super-target is itself const.
                if let Some(r) = resolve_super_access(node, file, ctx)
                    && target_is_const(r.node, ctx)
                {
                    return true;
                }
                let (is_const, _) = const_scope(node, file, ctx);
                if is_const {
                    let span = source_tree
                        .identifier(node)
                        .map(|i| source_tree.span(i))
                        .unwrap_or_else(|| source_tree.span(node));
                    holder.add_error(span, "cannot access from const");
                }
            }
            _ => {}
        }
        true
    }
}

fn check_recursive(
    access: NodeId,
    target: NodeRef,
    holder: &mut DiagnosticsHolder,
    file: pkl_types::FileId,
    ctx: &AnalysisContext,
) {
    let Some(source_tree) = ctx.tree(file) else {
        return;
    };
    let enclosing = source_tree.ancestor(access, |k| k.is_container());
    if let Some(enc) = enclosing
        && NodeRef::new(file, enc) == target
    {
        let span = source_tree
            .identifier(access)
            .map(|i| source_tree.span(i))
            .unwrap_or_else(|| source_tree.span(access));
        holder.add_error(span, "recursive property reference");
    };
}

fn check_const_access(
    access: NodeId,
    target: NodeRef,
    holder: &mut DiagnosticsHolder,
    file: pkl_types::FileId,
    ctx: &AnalysisContext,
) {
    let Some(source_tree) = ctx.tree(file) else {
        return;
    };
    if target_is_const(target, ctx) {
        return;
    }

    let (is_const, _) = const_scope(access, file, ctx);
    if is_const {
        let span = source_tree
            .identifier(access)
            .map(|i| source_tree.span(i))
            .unwrap_or_else(|| source_tree.span(access));
        holder.add_error(span, "cannot access from const");
    }
}

fn check_static_body_access(
    access: NodeId,
    target: NodeRef,
    holder: &mut DiagnosticsHolder,
    file: pkl_types::FileId,
    ctx: &AnalysisContext,
) {
    let Some(source_tree) = ctx.tree(file) else {
        return;
    };
    // find static ancestor
    let Some(static_parent) = source_tree.ancestor(access, |k| {
        matches!(
            k,
            NodeKind::Class | NodeKind::Annotation | NodeKind::TypeAlias
        )
    }) else {
        return;
    };

    // target must be a module-level non-const property/method
    // in the same file
    if target.file != file {
        return;
    }
    if let parent = source_tree.parent(target.value)
        && !matches!(parent.map(|p| source_tree.kind(p)), Some(NodeKind::Module))
    {
        return;
    }
    if source_tree.has_modifier(target.value, TokenType::Const) {
        return;
    }
    // check version for typealias
    if matches!(source_tree.kind(static_parent), NodeKind::TypeAlias)
        && effective_pkl_version(ctx, file) < Version::new(0, 26, 0)
    {
        holder.add_warning(
            source_tree.span(access),
            "should not access non-const property from typealias",
        );
        return;
    }

    holder.add_error(
        source_tree.span(access),
        "cannot access non-const property from static body",
    );
}

fn check_argument_count(
    access: NodeId,
    target: NodeRef,
    holder: &mut DiagnosticsHolder,
    file: pkl_types::FileId,
    ctx: &AnalysisContext,
) {
    let Some(source_tree) = ctx.tree(file) else {
        return;
    };
    let Some(tree) = ctx.tree(target.file) else {
        return;
    };

    if !matches!(
        tree.kind(target.value),
        NodeKind::ClassMethod | NodeKind::ObjectMethod
    ) {
        return;
    }

    let Some(method_header) = nodes::ClassMethod::cast(tree, target.value)
        .and_then(|method| method.header(tree))
        .or_else(|| {
            nodes::ObjectMethod::cast(tree, target.value).and_then(|method| method.header(tree))
        })
    else {
        return;
    };

    let param_list =
        nodes::MethodHeader::cast(tree, method_header).and_then(|header| header.parameters(tree));

    let declared_params = match param_list {
        Some(pl) => {
            nodes::ParameterList::cast(tree, pl).map_or(0, |list| list.parameters(tree).count())
        }
        None => 0,
    };

    let Some(arg_list) = nodes::QualifiedAccessExpr::cast(source_tree, access)
        .and_then(|expr| expr.arguments(source_tree))
        .or_else(|| {
            nodes::UnqualifiedAccessExpr::cast(source_tree, access)
                .and_then(|expr| expr.arguments(source_tree))
        })
        .or_else(|| {
            nodes::SuperAccessExpr::cast(source_tree, access)
                .and_then(|expr| expr.arguments(source_tree))
        })
    else {
        return;
    };
    let args = nodes::ArgumentList::cast(source_tree, arg_list)
        .map_or(0, |list| list.arguments(source_tree).count());
    if args == declared_params {
        return;
    }
    if is_vararg(&target, ctx) {
        return;
    }
    if args < declared_params {
        holder.add_error(
            source_tree.span(access),
            format!("too few arguments (expected {declared_params}, got {args})"),
        );
    } else {
        holder.add_error(
            source_tree.span(access),
            format!("too many arguments (expected {declared_params}, got {args})"),
        );
    }
}

fn is_vararg(method_decl: &NodeRef, ctx: &AnalysisContext) -> bool {
    let Some(tree) = ctx.tree(method_decl.file) else {
        return false;
    };
    let annotation_children = nodes::ClassMethod::cast(tree, method_decl.value)
        .into_iter()
        .flat_map(|method| method.annotations(tree));
    for annotation in annotation_children {
        let Some(name_node) =
            nodes::Annotation::cast(tree, annotation).and_then(|annotation| annotation.name(tree))
        else {
            continue;
        };

        let name_parts = tree.qualified_identifier_segments(name_node);
        let Some(class_node_ref) =
            resolve_type_name(ctx, &name_parts, NodeRef::new(method_decl.file, name_node))
        else {
            continue;
        };
        let class_type = Type::class(class_node_ref);
        if class_type.class_equals(ctx.base.varargs_type()) {
            return true;
        }
    }
    false
}

fn target_is_const(target: NodeRef, ctx: &AnalysisContext) -> bool {
    let Some(tree) = ctx.tree(target.file) else {
        return false;
    };
    tree.has_modifier(target.value, TokenType::Const)
}

/// Returns `(is_const, is_in_const_scope)` for the given access node.
fn const_scope(access: NodeId, file: pkl_types::FileId, ctx: &AnalysisContext) -> (bool, bool) {
    let Some(source_tree) = ctx.tree(file) else {
        return (false, false);
    };
    let mut current = source_tree.parent(access);
    while let Some(id) = current {
        match source_tree.kind(id) {
            NodeKind::ClassProperty | NodeKind::ClassMethod => {
                return (source_tree.has_modifier(id, TokenType::Const), false);
            }
            NodeKind::ObjectBody => {
                let mut inner = source_tree.parent(id);
                while let Some(iid) = inner {
                    match source_tree.kind(iid) {
                        NodeKind::ClassProperty | NodeKind::ClassMethod => {
                            let c = source_tree.has_modifier(iid, TokenType::Const);
                            return (c, c);
                        }
                        _ => inner = source_tree.parent(iid),
                    }
                }
                return (false, false);
            }
            _ => current = source_tree.parent(id),
        }
    }
    (false, false)
}

/// Resolve a `SuperAccessExpr` to the inherited member it refers to.
fn resolve_super_access(
    access: NodeId,
    file: pkl_types::FileId,
    ctx: &AnalysisContext,
) -> Option<ResolvedType> {
    let source_tree = ctx.tree(file)?;
    let name = source_tree.name(access)?;
    let super_ty = pkl_types::resolver::access_receiver_type(NodeRef::new(file, access), ctx)?;
    find_member(&super_ty, name, ctx)
}

/// If the outer access (`outer_target`) resolves to one of the `List` /
/// `Map` members that are also available on the source `Listing` / `Mapping`,
/// and the receiver expression is itself a call to the corresponding
/// `Listing.toList` / `Mapping.toMap` method, emit a hint flagging the
/// conversion as redundant.
fn check_redundant_conversion(
    receiver: NodeId,
    receiver_type: &Type,
    outer_target: NodeRef,
    holder: &mut DiagnosticsHolder,
    file: pkl_types::FileId,
    ctx: &AnalysisContext,
) {
    let Some(source_tree) = ctx.tree(file) else {
        return;
    };
    let base = ctx.base;
    let conversion_target = if receiver_type.class_equals(base.list_type())
        && (outer_target == base.list_join_method()
            || outer_target == base.list_is_distinct_by_method()
            || outer_target == base.list_fold_method()
            || outer_target == base.list_fold_indexed_method()
            || outer_target == base.list_is_empty_property()
            || outer_target == base.list_length_property()
            || outer_target == base.list_is_distinct_property())
    {
        base.listing_to_list_method()
    } else if receiver_type.class_equals(base.map_type())
        && (outer_target == base.map_contains_key_method()
            || outer_target == base.map_get_or_null_method()
            || outer_target == base.map_fold_method()
            || outer_target == base.map_is_empty_property()
            || outer_target == base.map_length_property()
            || outer_target == base.map_keys_property())
    {
        base.mapping_to_map_method()
    } else {
        return;
    };

    // The receiver must be a QualifiedAccessExpr whose resolved member is
    // exactly the conversion method (e.g. `Listing.toList`).
    if !matches!(source_tree.kind(receiver), NodeKind::QualifiedAccessExpr) {
        return;
    }
    let Some(receiver_receiver) = nodes::QualifiedAccessExpr::cast(source_tree, receiver)
        .and_then(|expr| expr.receiver(source_tree))
    else {
        return;
    };
    let Some(receiver_name) = source_tree.name(receiver) else {
        return;
    };
    let receiver_receiver_type = NodeRef::new(file, receiver_receiver)
        .cast(ctx, pkl_syntax::ast::nodes::Expr::cast)
        .map_or(Type::Unknown, |expr| pkl_types::expr_type(expr, ctx));
    let Some(receiver_resolved) = find_member(&receiver_receiver_type, receiver_name, ctx) else {
        return;
    };
    if receiver_resolved.node != conversion_target {
        return;
    }

    let span = source_tree
        .identifier(receiver)
        .map(|i| source_tree.span(i))
        .unwrap_or_else(|| source_tree.span(receiver));
    let conversion_name = if conversion_target == base.listing_to_list_method() {
        "toList"
    } else {
        "toMap"
    };
    holder.add_hint(
        span,
        format!(
            "redundant conversion `.{conversion_name}()` — method also available on the source"
        ),
    );
}
