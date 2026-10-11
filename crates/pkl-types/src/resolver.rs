//! Name resolution and class hierarchy queries.

use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};

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
    resolve_type_name(ctx, &name_parts, node)
}

pub fn resolve_type_name(ctx: &AnalysisContext, parts: &[&str], node: NodeRef) -> Option<NodeRef> {
    let name = *parts.last()?;
    let tree = ctx.tree_for(node.file_id)?;
    if parts.len() == 1 {
        if let Some(tp) = resolve_local_type_parameter(node, name, ctx) {
            return Some(tp);
        }
        let mut module = Some(NodeRef::new(node.file_id, tree.root()));
        let mut seen = std::collections::HashSet::new();
        while let Some(root) = module {
            if !seen.insert(root) {
                break;
            }
            let Some(t) = ctx.tree_for(root.file_id) else {
                break;
            };
            if let Some(id) = resolve_type_name_in_module(t, name) {
                return Some(root.with_node(id));
            }
            module = module_super(root, ctx);
        }
        return resolve_type_name_in_module(ctx.base.tree(), name)
            .map(|id| NodeRef::new(FileId::BASE, id));
    }
    let import = find_import(tree, parts[0])?;
    let mut module = ctx
        .imports
        .get(&NodeRef::new(node.file_id, import))?
        .single()?;
    let mut seen = std::collections::HashSet::new();
    while seen.insert(module) {
        let t = ctx.tree_for(module.file_id)?;
        if let Some(id) = resolve_type_name_in_module(t, name) {
            return Some(module.with_node(id));
        }
        module = module_super(module, ctx)?;
    }
    None
}

pub fn module_type(module_node: NodeRef, ctx: &AnalysisContext) -> Type {
    Type::Module {
        module_node,
        reference_name: ctx
            .tree_for(module_node.file_id)
            .and_then(|t| t.name(module_node.node_id))
            .unwrap_or("")
            .to_string(),
        constraints: vec![],
    }
}

pub fn module_super(module: NodeRef, ctx: &AnalysisContext) -> Option<NodeRef> {
    let tree = ctx.tree_for(module.file_id)?;
    let header = tree.child(module.node_id, |k| matches!(k, NodeKind::ModuleHeader))?;
    let clause = tree.child(header, |k| matches!(k, NodeKind::ModuleExtendsAmendsClause))?;
    ctx.imports.get(&module.with_node(clause))?.single()
}

pub fn import_name(tree: &SyntaxTree, import: NodeId) -> Option<String> {
    if let Some(id) = tree.identifier(import) {
        return Some(tree.text(id).to_string());
    }
    let uri = tree.child(import, |k| k.is_string_literal())?;
    let uri = tree.escaped_string_literal(uri)?;
    let name = uri
        .rsplit('/')
        .next()?
        .rsplit(':')
        .next()?
        .split(['?', '#'])
        .next()?;
    Some(name.strip_suffix(".pkl").unwrap_or(name).to_string())
}

fn find_import(tree: &SyntaxTree, name: &str) -> Option<NodeId> {
    tree.children(tree.root()).iter().copied().find(|&id| {
        matches!(tree.kind(id), NodeKind::Import | NodeKind::ImportGlob)
            && import_name(tree, id).as_deref() == Some(name)
    })
}

pub fn import_type(node: NodeRef, ctx: &AnalysisContext) -> Type {
    match ctx.imports.get(&node) {
        Some(crate::ImportResolution::Single(root)) => module_type(*root, ctx),
        Some(crate::ImportResolution::Glob(roots)) => {
            let value = roots
                .iter()
                .map(|r| module_type(*r, ctx))
                .reduce(|a, b| Type::Union {
                    left: Box::new(a),
                    right: Box::new(b),
                    constraints: vec![],
                })
                .unwrap_or(Type::Nothing);
            ctx.base
                .mapping_type()
                .clone()
                .with_type_args(vec![ctx.base.string_type().clone(), value])
        }
        None | Some(crate::ImportResolution::Unresolved { .. }) => Type::Unknown,
    }
}

pub fn is_subclass(mut a: NodeRef, b: NodeRef, ctx: &AnalysisContext) -> bool {
    let mut seen = std::collections::HashSet::new();
    while seen.insert(a) {
        if a == b {
            return true;
        }
        let Some(tree) = ctx.tree_for(a.file_id) else {
            return false;
        };
        let Some(target) = tree.class_extends_target(a.node_id) else {
            return false;
        };
        let Some(parent) = resolve_type_name(
            ctx,
            &tree.qualified_identifier_segments(target),
            a.with_node(target),
        ) else {
            return false;
        };
        a = parent;
    }
    false
}

fn kind_is_class_or_alias(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Class | NodeKind::TypeAlias)
}

/// Resolve the reference under the cursor at `(line, col)`.
///
/// Returns the declaration `node` refers to, or `None` if there's nothing
/// to resolve at that position (cursor is on whitespace, a keyword, a
/// literal, etc.). The returned `NodeRef` may live in a different file than
/// `node` — `String` in a user file resolves to a class in `pkl:base`, an
/// imported identifier resolves into the imported module.
pub fn resolve_reference(
    node: NodeRef,
    line: u32,
    col: u32,
    ctx: &AnalysisContext,
) -> Option<NodeRef> {
    let tree = ctx.tree_for(node.file_id)?;
    let ctx = &AnalysisContext::new(tree, node.file_id, ctx.base, ctx.trees, ctx.imports);

    // We don't want to find terminals, so we go up to the first non-terminal node
    let mut node_id = node.node_id;
    while tree.kind(node_id).is_terminal() {
        node_id = tree.parent(node_id)?;
    }

    let bindings = TypeBindings::new();
    match tree.kind(node_id) {
        NodeKind::UnqualifiedAccessExpr => {
            let name = tree.name(node_id)?;
            resolve_unqualified_access(node_id, name, &bindings, ctx).map(|r| r.node)
        }
        NodeKind::QualifiedAccessExpr | NodeKind::SuperAccessExpr => {
            let name = tree.name(node_id)?;
            let receiver_ty = access_receiver_type(node_id, &bindings, ctx)?;
            find_member_on_type(&receiver_ty, name, &bindings, ctx).map(|r| r.node)
        }
        NodeKind::DeclaredType => resolve_declared_type(ctx, node.with_node(node_id)),
        NodeKind::QualifiedIdentifier => {
            let segments = tree.qualified_identifier_segments(node_id);
            if segments.len() > 1
                && let Some(import) = find_import(tree, segments[0])
                && let Some(identifier) = tree
                    .children(node_id)
                    .iter()
                    .find(|&&id| tree.kind(id).is_terminal() && tree.span(id).matches(line, col))
                && tree.text(*identifier) == segments[0]
            {
                return ctx
                    .imports
                    .get(&NodeRef::new(node.file_id, import))
                    .and_then(crate::ImportResolution::single);
            }
            let parent = tree.parent(node_id)?;
            match tree.kind(parent) {
                NodeKind::DeclaredType => resolve_declared_type(ctx, node.with_node(parent)),
                NodeKind::ClassExtendsClause => {
                    let segments = tree.qualified_identifier_segments(node_id);
                    resolve_type_name(ctx, &segments, node.with_node(node_id))
                }
                _ => None,
            }
        }

        NodeKind::Import | NodeKind::ImportGlob => {
            let alias = tree.identifier(node_id)?;
            if tree.span(alias).matches(line, col) {
                return ctx
                    .imports
                    .get(&node.with_node(node_id))
                    .and_then(crate::ImportResolution::single);
            }
            None
        }
        NodeKind::StringConstant => {
            let parent = tree.parent(node_id)?;
            match tree.kind(parent) {
                NodeKind::Import | NodeKind::ImportExpr | NodeKind::ModuleExtendsAmendsClause => {
                    ctx.imports
                        .get(&node.with_node(parent))
                        .and_then(crate::ImportResolution::single)
                }
                _ => None,
            }
        }
        // TODO(property-self-resolve)
        // TODO(doc-comment-links)
        _ => None,
    }
}

/// Find the `TypeAnnotation` child of a class/object member declaration.
fn member_type_annotation(tree: &SyntaxTree, member: NodeId) -> Option<NodeId> {
    let holder = match tree.kind(member) {
        NodeKind::ClassMethod | NodeKind::ObjectMethod => tree
            .child(member, |k| matches!(k, NodeKind::MethodHeader))
            .unwrap_or(member),
        _ => member,
    };
    tree.child(holder, |k| matches!(k, NodeKind::TypeAnnotation))
}

/// Look up a member by name on a type. Returns the member's resolved type if
/// found, or `None` otherwise.
pub fn find_member_on_type(
    ty: &Type,
    name: &str,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<ResolvedType> {
    find_member_inner(
        ty,
        name,
        bindings,
        ctx,
        &mut std::collections::HashSet::new(),
    )
}

fn find_member_inner(
    ty: &Type,
    name: &str,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
    seen: &mut std::collections::HashSet<NodeRef>,
) -> Option<ResolvedType> {
    match ty {
        Type::Class {
            class_node,
            type_args,
            ..
        } => {
            if !seen.insert(*class_node) {
                return None;
            }
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
                let ty = definition_type(
                    NodeRef::new(class_node.file_id, class_member),
                    &member_bindings,
                    ctx,
                );
                let member_ref = NodeRef::new(class_node.file_id, class_member);

                Some(ResolvedType::new(member_ref, ty))
            } else {
                let extends_target = tree.class_extends_target(class_node.node_id)?;
                let super_type_name = tree.qualified_identifier_segments(extends_target);
                let super_type_node = resolve_type_name(
                    ctx,
                    &super_type_name,
                    NodeRef::new(class_node.file_id, extends_target),
                )?;

                find_member_inner(
                    &Type::class(super_type_node),
                    name,
                    &member_bindings,
                    ctx,
                    seen,
                )
            }
        }
        Type::Module { module_node, .. } => {
            if !seen.insert(*module_node) {
                return None;
            }
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
                let member_ref = NodeRef::new(module_node.file_id, module_member);
                let ty = definition_type(member_ref, bindings, ctx);
                Some(ResolvedType::new(member_ref, ty))
            } else {
                if let Some(parent) = module_super(*module_node, ctx) {
                    find_member_inner(&module_type(parent, ctx), name, bindings, ctx, seen)
                } else {
                    find_member_inner(ctx.base.module_type(), name, bindings, ctx, seen)
                }
            }
        }
        Type::Alias { .. } => find_member_inner(&ty.unaliased(ctx), name, bindings, ctx, seen),
        Type::Union { left, right, .. } => {
            let left_member = find_member_inner(left, name, bindings, ctx, seen);
            seen.clear();
            let right_member = find_member_inner(right, name, bindings, ctx, seen);
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

pub fn definition_type(node: NodeRef, bindings: &TypeBindings, ctx: &AnalysisContext) -> Type {
    let Some(tree) = ctx.tree_for(node.file_id) else {
        return Type::Unknown;
    };
    let local = AnalysisContext::new(tree, node.file_id, ctx.base, ctx.trees, ctx.imports);
    match tree.kind(node.node_id) {
        NodeKind::Class => return Type::class(node),
        NodeKind::TypeAlias => {
            return Type::Alias {
                alias_node: node,
                type_args: vec![],
                constraints: vec![],
            };
        }
        _ => {}
    }
    if let Some(ann) =
        member_type_annotation(tree, node.node_id).and_then(|a| tree.child(a, |k| k.is_type_node()))
    {
        return to_type(node.with_node(ann), bindings, &local);
    }
    tree.child(node.node_id, |k| k.is_expr_node())
        .map(|e| compute_expr_type(e, bindings, &local))
        .unwrap_or(Type::Unknown)
}

pub fn super_type(ty: &Type, ctx: &AnalysisContext) -> Option<Type> {
    match ty {
        Type::Module { module_node, .. } => {
            module_super(*module_node, ctx).map(|m| module_type(m, ctx))
        }
        Type::Class { class_node, .. } => {
            let tree = ctx.tree_for(class_node.file_id)?;
            let target = tree.class_extends_target(class_node.node_id)?;
            resolve_type_name(
                ctx,
                &tree.qualified_identifier_segments(target),
                class_node.with_node(target),
            )
            .map(Type::class)
        }
        _ => None,
    }
}

/// Whether `node` is inside the body of this let expression, not its initializer.
pub fn let_binding_is_visible(let_node: NodeId, node: NodeId, ctx: &AnalysisContext) -> bool {
    let exprs = ctx.tree.children_matching(let_node, |k| k.is_expr_node());
    let Some(&body) = exprs.get(1) else {
        return false;
    };
    let mut current = Some(node);
    while let Some(id) = current {
        if id == body {
            return true;
        }
        if id == let_node {
            return false;
        }
        current = ctx.tree.parent(id);
    }
    false
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
            NodeKind::LetExpr if let_binding_is_visible(id, node, ctx) => {
                match_let_binding(id, name, bindings, ctx)
            }
            NodeKind::FunctionLiteralExpr => {
                let param_list = ctx.tree.child(id, |k| matches!(k, NodeKind::ParameterList));
                param_list.and_then(|pl| match_param(pl, name, bindings, ctx))
            }
            NodeKind::ClassMethod | NodeKind::ObjectMethod => {
                let param_list = ctx
                    .tree
                    .child(id, |k| matches!(k, NodeKind::MethodHeader))
                    .and_then(|mh| ctx.tree.child(mh, |k| matches!(k, NodeKind::ParameterList)));
                param_list.and_then(|pl| match_param(pl, name, bindings, ctx))
            }
            NodeKind::ObjectBody => ctx
                .tree
                .children_matching(id, |k| {
                    matches!(k, NodeKind::ObjectProperty | NodeKind::ObjectMethod)
                })
                .into_iter()
                .find(|&m| ctx.tree.name(m) == Some(name))
                .map(|m| {
                    let r = NodeRef::new(ctx.file_id, m);
                    ResolvedType::new(r, definition_type(r, bindings, ctx))
                }),
            _ => None,
        };
        if hit.is_some() {
            return hit;
        }
        current = ctx.tree.parent(id);
    }
    None
}

fn resolve_local_type_parameter(
    node: NodeRef,
    name: &str,
    ctx: &AnalysisContext,
) -> Option<NodeRef> {
    // Walk ancestors in the file the `node` lives in — NOT `ctx.tree`. The
    // resolver can be invoked across files when find_member_on_type follows
    // an extends chain into pkl:base, so the use-site tree differs from the
    // user's tree.
    let tree = ctx.tree_for(node.file_id)?;
    let mut current = tree.parent(node.node_id);
    while let Some(id) = current {
        if matches!(
            tree.kind(id),
            NodeKind::Class | NodeKind::TypeAlias | NodeKind::ClassMethod
        ) {
            let holder = if matches!(tree.kind(id), NodeKind::ClassMethod) {
                tree.child(id, |k| matches!(k, NodeKind::MethodHeader))
                    .unwrap_or(id)
            } else {
                id
            };
            if let Some(param_list) =
                tree.child(holder, |k| matches!(k, NodeKind::TypeParameterList))
                && let Some(tp) = tree
                    .children_matching(param_list, |k| matches!(k, NodeKind::TypeParameter))
                    .into_iter()
                    .find(|&n| tree.name(n) == Some(name))
            {
                return Some(NodeRef::new(node.file_id, tp));
            }
        }
        current = tree.parent(id);
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
    let receiver = compute_this_type(node, bindings, ctx);
    for ty in [
        &receiver,
        &module_type(NodeRef::new(ctx.file_id, ctx.tree.root()), ctx),
    ] {
        let (root, body) = match ty {
            Type::Module { module_node, .. } => (*module_node, module_node.node_id),
            Type::Class { class_node, .. } => {
                let tree = ctx.tree_for(class_node.file_id)?;
                (
                    *class_node,
                    tree.child(class_node.node_id, |k| matches!(k, NodeKind::ClassBody))
                        .unwrap_or(class_node.node_id),
                )
            }
            _ => continue,
        };
        let tree = ctx.tree_for(root.file_id)?;
        if let Some(member) = tree.children(body).iter().copied().find(|&id| {
            matches!(
                tree.kind(id),
                NodeKind::ClassProperty
                    | NodeKind::ClassMethod
                    | NodeKind::Class
                    | NodeKind::TypeAlias
            ) && tree.name(id) == Some(name)
        }) {
            let r = root.with_node(member);
            return Some(ResolvedType::new(r, definition_type(r, bindings, ctx)));
        }
    }
    if let Some(import) = find_import(ctx.tree, name) {
        let declaration = NodeRef::new(ctx.file_id, import);
        let target = ctx
            .imports
            .get(&declaration)
            .and_then(crate::ImportResolution::single)
            .unwrap_or(declaration);
        return Some(ResolvedType::new(target, import_type(declaration, ctx)));
    }
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

/// Compute the type of the receiver of an access expression.
pub fn access_receiver_type(
    access: NodeId,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<Type> {
    match ctx.tree.kind(access) {
        NodeKind::UnqualifiedAccessExpr => Some(compute_this_type(access, bindings, ctx)),
        NodeKind::QualifiedAccessExpr => {
            let receiver = ctx.tree.child(access, |k| k.is_expr_node())?;
            Some(compute_expr_type(receiver, bindings, ctx))
        }
        NodeKind::SuperAccessExpr => super_type(&compute_this_type(access, bindings, ctx), ctx),
        _ => None,
    }
}

/// Return the key type of a container-shaped class type at this position.
///
/// - `Listing<E>` / `List<E>` → `Int` (indices)
/// - `Mapping<K,V>` / `Map<K,V>` → `K`
/// - Anything else → `None`
pub fn container_key_type(ty: &Type, ctx: &AnalysisContext) -> Option<Type> {
    let Type::Class { type_args, .. } = ty else {
        return None;
    };
    if ty.is_subtype_of(ctx.base.mapping_type(), ctx) || ty.is_subtype_of(ctx.base.map_type(), ctx)
    {
        type_args.first().cloned()
    } else if ty.is_subtype_of(ctx.base.listing_type(), ctx)
        || ty.is_subtype_of(ctx.base.list_type(), ctx)
    {
        Some(ctx.base.int_type().clone())
    } else {
        None
    }
}

/// Return the value (element) type of a container-shaped class type at this
/// position.
///
/// - `Listing<E>` / `List<E>` / `Set<E>` → `E`
/// - `Mapping<K,V>` / `Map<K,V>` → `V`
/// - Anything else → `None`
pub fn container_value_type(ty: &Type, ctx: &AnalysisContext) -> Option<Type> {
    let Type::Class { type_args, .. } = ty else {
        return None;
    };
    if ty.is_subtype_of(ctx.base.mapping_type(), ctx) || ty.is_subtype_of(ctx.base.map_type(), ctx)
    {
        type_args.get(1).cloned()
    } else if ty.is_subtype_of(ctx.base.listing_type(), ctx)
        || ty.is_subtype_of(ctx.base.list_type(), ctx)
        || ty.is_subtype_of(ctx.base.set_type(), ctx)
    {
        type_args.first().cloned()
    } else {
        None
    }
}
