//! Name resolution and class hierarchy queries.

use pkl_syntax::TextSize;
use pkl_syntax::ast::nodes;
use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};

use crate::compute_expr::{compute_expr_type, compute_this_type};
use crate::context::AnalysisContext;
use crate::file_id::NodeRef;
use crate::to_type::to_type_with_bindings;
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
    nodes::Module::cast(tree, tree.root())?
        .members(tree)
        .filter(|&id| kind_is_class_or_alias(tree.kind(id)))
        .find(|&id| tree.name(id) == Some(name))
}

/// Look up the definition of a declared type
pub fn resolve_declared_type(ctx: &AnalysisContext, node: NodeRef) -> Option<NodeRef> {
    let tree = ctx.tree(node.file)?;
    let name_parts = tree.declared_type_parts(node.value);
    resolve_type_name(ctx, &name_parts, node)
}

pub fn resolve_type_name(ctx: &AnalysisContext, parts: &[&str], node: NodeRef) -> Option<NodeRef> {
    let name = *parts.last()?;
    let tree = ctx.tree(node.file)?;
    if parts.len() == 1 {
        if let Some(tp) = resolve_local_type_parameter(node, name, ctx) {
            return Some(tp);
        }
        let mut module = Some(NodeRef::new(node.file, tree.root()));
        let mut seen = std::collections::HashSet::new();
        while let Some(root) = module {
            if !seen.insert(root) {
                break;
            }
            let Some(t) = ctx.tree(root.file) else {
                break;
            };
            if let Some(id) = resolve_type_name_in_module(t, name) {
                return Some(root.with_value(id));
            }
            module = module_super(root, ctx);
        }
        return resolve_type_name_in_module(ctx.tree(ctx.base.file_id())?, name)
            .map(|id| NodeRef::new(ctx.base.file_id(), id));
    }
    let import = find_import(tree, parts[0])?;
    let mut module = ctx.import(NodeRef::new(node.file, import))?.single()?;
    let mut seen = std::collections::HashSet::new();
    while seen.insert(module) {
        let t = ctx.tree(module.file)?;
        if let Some(id) = resolve_type_name_in_module(t, name) {
            return Some(module.with_value(id));
        }
        module = module_super(module, ctx)?;
    }
    None
}

pub fn module_type(module_node: NodeRef, _ctx: &AnalysisContext) -> Type {
    Type::Module { module_node }
}

pub fn module_super(module: NodeRef, ctx: &AnalysisContext) -> Option<NodeRef> {
    let tree = ctx.tree(module.file)?;
    let header = nodes::Module::cast(tree, module.value)?.header(tree)?;
    let clause = nodes::ModuleHeader::cast(tree, header)?.extends_amends_clause(tree)?;
    ctx.import(module.with_value(clause))?.single()
}

pub fn import_name(tree: &SyntaxTree, import: NodeId) -> Option<String> {
    if let Some(id) = tree.identifier(import) {
        return Some(tree.text(id).to_string());
    }
    let uri = nodes::Import::cast(tree, import)
        .and_then(|n| n.uri(tree))
        .or_else(|| nodes::ImportGlob::cast(tree, import).and_then(|n| n.uri(tree)))?;
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
    nodes::Module::cast(tree, tree.root())?
        .imports(tree)
        .find(|&id| import_name(tree, id).as_deref() == Some(name))
}

pub fn import_type(node: NodeRef, ctx: &AnalysisContext) -> Type {
    match ctx.import(node) {
        Some(crate::ImportResolution::Single(root)) => module_type(root, ctx),
        Some(crate::ImportResolution::Glob(roots)) => {
            let value = Type::union(roots.iter().map(|r| module_type(*r, ctx)), ctx);
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
        let Some(tree) = ctx.tree(a.file) else {
            return false;
        };
        let Some(target) = nodes::Class::cast(tree, a.value).and_then(|n| n.extends_target(tree))
        else {
            return false;
        };
        let Some(parent) = resolve_type_name(
            ctx,
            &tree.qualified_identifier_segments(target),
            a.with_value(target),
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

/// Resolve the reference under the cursor at the byte `offset`.
///
/// Returns the declaration `node` refers to, or `None` if there's nothing
/// to resolve at that position (cursor is on whitespace, a keyword, a
/// literal, etc.). The returned `NodeRef` may live in a different file than
/// `node` — `String` in a user file resolves to a class in `pkl:base`, an
/// imported identifier resolves into the imported module.
pub fn resolve_reference(
    node: NodeRef,
    offset: TextSize,
    ctx: &AnalysisContext,
) -> Option<NodeRef> {
    let tree = ctx.tree(node.file)?;

    // We don't want to find terminals, so we go up to the first non-terminal node
    let mut node_id = node.value;
    while tree.kind(node_id).is_terminal() {
        node_id = tree.parent(node_id)?;
    }

    let bindings = TypeBindings::new();
    match tree.kind(node_id) {
        NodeKind::UnqualifiedAccessExpr => {
            let name = tree.name(node_id)?;
            resolve_unqualified_access_with_bindings(node.with_value(node_id), name, &bindings, ctx)
                .map(|r| r.node)
        }
        NodeKind::QualifiedAccessExpr | NodeKind::SuperAccessExpr => {
            let name = tree.name(node_id)?;
            let receiver_ty =
                access_receiver_type_with_bindings(node.with_value(node_id), &bindings, ctx)?;
            find_member_on_type(&receiver_ty, name, &bindings, ctx).map(|r| r.node)
        }
        NodeKind::DeclaredType => resolve_declared_type(ctx, node.with_value(node_id)),
        NodeKind::QualifiedIdentifier => {
            let segments = tree.qualified_identifier_segments(node_id);
            if segments.len() > 1
                && let Some(import) = find_import(tree, segments[0])
                && let Some(identifier) = nodes::QualifiedIdentifier::cast(tree, node_id)?
                    .identifiers(tree)
                    .find(|&id| tree.span(id).contains_inclusive(offset))
                && tree.text(identifier) == segments[0]
            {
                return ctx
                    .import(NodeRef::new(node.file, import))
                    .and_then(|resolution| resolution.single());
            }
            let parent = tree.parent(node_id)?;
            match tree.kind(parent) {
                NodeKind::DeclaredType => resolve_declared_type(ctx, node.with_value(parent)),
                NodeKind::ClassExtendsClause => {
                    let segments = tree.qualified_identifier_segments(node_id);
                    resolve_type_name(ctx, &segments, node.with_value(node_id))
                }
                _ => None,
            }
        }

        NodeKind::Import | NodeKind::ImportGlob => {
            let alias = tree.identifier(node_id)?;
            if tree.span(alias).contains_inclusive(offset) {
                return ctx
                    .import(node.with_value(node_id))
                    .and_then(|resolution| resolution.single());
            }
            None
        }
        NodeKind::StringConstant => {
            let parent = tree.parent(node_id)?;
            match tree.kind(parent) {
                NodeKind::Import | NodeKind::ImportExpr | NodeKind::ModuleExtendsAmendsClause => {
                    ctx.import(node.with_value(parent))
                        .and_then(|resolution| resolution.single())
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
pub(crate) fn member_type_annotation(tree: &SyntaxTree, member: NodeId) -> Option<NodeId> {
    nodes::ClassProperty::cast(tree, member)
        .and_then(|n| n.type_annotation(tree))
        .or_else(|| nodes::ObjectProperty::cast(tree, member).and_then(|n| n.type_annotation(tree)))
        .or_else(|| {
            nodes::TypedIdentifier::cast(tree, member).and_then(|n| n.type_annotation(tree))
        })
        .or_else(|| {
            method_header(tree, member)
                .and_then(|n| nodes::MethodHeader::cast(tree, n)?.type_annotation(tree))
        })
}

fn method_header(tree: &SyntaxTree, member: NodeId) -> Option<NodeId> {
    nodes::ClassMethod::cast(tree, member)
        .and_then(|n| n.header(tree))
        .or_else(|| nodes::ObjectMethod::cast(tree, member).and_then(|n| n.header(tree)))
}

fn member_value(tree: &SyntaxTree, member: NodeId) -> Option<NodeId> {
    nodes::ClassProperty::cast(tree, member)
        .and_then(|n| n.value(tree))
        .or_else(|| nodes::ObjectProperty::cast(tree, member).and_then(|n| n.value(tree)))
        .or_else(|| nodes::ClassMethod::cast(tree, member).and_then(|n| n.body(tree)))
        .or_else(|| nodes::ObjectMethod::cast(tree, member).and_then(|n| n.body(tree)))
}

/// Look up a member by name on a type. Returns the member's resolved type if
/// found, or `None` otherwise.
pub(crate) fn find_member_on_type(
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
    match ty.unconstrained() {
        Type::Class {
            class_node,
            type_args,
            ..
        } => {
            if !seen.insert(*class_node) {
                return None;
            }
            let tree = ctx.tree(class_node.file)?;
            let member_bindings = class_member_bindings(*class_node, type_args, bindings, ctx);
            let mut members = nodes::Class::cast(tree, class_node.value)
                .and_then(|n| n.body(tree))
                .and_then(|n| nodes::ClassBody::cast(tree, n))
                .into_iter()
                .flat_map(|n| n.members(tree));
            if let Some(class_member) = members.find(|&n| tree.name(n) == Some(name)) {
                let ty = definition_type_with_bindings(
                    NodeRef::new(class_node.file, class_member),
                    &member_bindings,
                    ctx,
                );
                let member_ref = NodeRef::new(class_node.file, class_member);

                Some(ResolvedType::new(member_ref, ty))
            } else {
                let extends_target =
                    nodes::Class::cast(tree, class_node.value)?.extends_target(tree)?;
                let super_type_name = tree.qualified_identifier_segments(extends_target);
                let super_type_node = resolve_type_name(
                    ctx,
                    &super_type_name,
                    NodeRef::new(class_node.file, extends_target),
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
            let tree = ctx.tree(module_node.file)?;
            if let Some(module_member) = nodes::Module::cast(tree, module_node.value)?
                .members(tree)
                .find(|&n| tree.name(n) == Some(name))
            {
                let member_ref = NodeRef::new(module_node.file, module_member);
                let ty = definition_type_with_bindings(member_ref, bindings, ctx);
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
        Type::Union { members } => {
            let mut node = None;
            let mut types = Vec::new();
            for member in members.iter() {
                seen.clear();
                if let Some(resolved) = find_member_inner(member, name, bindings, ctx, seen) {
                    node.get_or_insert(resolved.node);
                    types.push(resolved.ty);
                }
            }
            node.map(|node| ResolvedType::new(node, Type::union(types, ctx)))
        }
        _ => None,
    }
}

pub(crate) fn definition_type_with_bindings(
    node: NodeRef,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Type {
    let Some(tree) = ctx.tree(node.file) else {
        return Type::Unknown;
    };
    match tree.kind(node.value) {
        NodeKind::Class => return Type::class(node),
        NodeKind::TypeAlias => {
            return Type::Alias {
                alias_node: node,
                type_args: vec![],
            };
        }
        _ => {}
    }
    if let Some(ann) = member_type_annotation(tree, node.value)
        .and_then(|a| nodes::TypeAnnotation::cast(tree, a)?.ty(tree))
    {
        return to_type_with_bindings(node.with_value(ann), bindings, ctx);
    }
    member_value(tree, node.value)
        .map(|e| compute_expr_type(node.with_value(e), bindings, ctx))
        .unwrap_or(Type::Unknown)
}

pub fn super_type(ty: &Type, ctx: &AnalysisContext) -> Option<Type> {
    match ty.unconstrained() {
        Type::Module { module_node, .. } => {
            module_super(*module_node, ctx).map(|m| module_type(m, ctx))
        }
        Type::Class { class_node, .. } => {
            let tree = ctx.tree(class_node.file)?;
            let target = nodes::Class::cast(tree, class_node.value)?.extends_target(tree)?;
            resolve_type_name(
                ctx,
                &tree.qualified_identifier_segments(target),
                class_node.with_value(target),
            )
            .map(Type::class)
        }
        _ => None,
    }
}

/// Whether `node` is inside the body of this let expression, not its initializer.
pub fn let_binding_is_visible(let_node: NodeRef, node: NodeRef, ctx: &AnalysisContext) -> bool {
    if let_node.file != node.file {
        return false;
    }
    let Some(tree) = ctx.tree(node.file) else {
        return false;
    };
    let let_node = let_node.value;
    let node = node.value;
    let Some(body) = nodes::LetExpr::cast(tree, let_node).and_then(|n| n.body(tree)) else {
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
        current = tree.parent(id);
    }
    false
}

/// Walk the AST ancestors of `node` looking for a binding named `name`.
pub(crate) fn resolve_local_binding(
    node: NodeRef,
    name: &str,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<ResolvedType> {
    let file = node.file;
    let tree = ctx.tree(file)?;
    let node = node.value;
    let mut current = tree.parent(node);
    while let Some(id) = current {
        let hit = match tree.kind(id) {
            NodeKind::LetExpr
                if let_binding_is_visible(
                    NodeRef::new(file, id),
                    NodeRef::new(file, node),
                    ctx,
                ) =>
            {
                match_let_binding(NodeRef::new(file, id), name, bindings, ctx)
            }
            NodeKind::FunctionLiteralExpr => {
                let param_list =
                    nodes::FunctionLiteralExpr::cast(tree, id).and_then(|n| n.parameters(tree));
                param_list.and_then(|pl| match_param(NodeRef::new(file, pl), name, bindings, ctx))
            }
            NodeKind::ClassMethod | NodeKind::ObjectMethod => {
                let param_list = method_header(tree, id)
                    .and_then(|n| nodes::MethodHeader::cast(tree, n)?.parameters(tree));
                param_list.and_then(|pl| match_param(NodeRef::new(file, pl), name, bindings, ctx))
            }
            NodeKind::ObjectBody => nodes::ObjectBody::cast(tree, id)
                .into_iter()
                .flat_map(|n| n.members(tree))
                .find(|&m| tree.name(m) == Some(name))
                .map(|m| {
                    let r = NodeRef::new(file, m);
                    ResolvedType::new(r, definition_type_with_bindings(r, bindings, ctx))
                }),
            _ => None,
        };
        if hit.is_some() {
            return hit;
        }
        current = tree.parent(id);
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
    let tree = ctx.tree(node.file)?;
    let mut current = tree.parent(node.value);
    while let Some(id) = current {
        if matches!(
            tree.kind(id),
            NodeKind::Class | NodeKind::TypeAlias | NodeKind::ClassMethod
        ) {
            let param_list = nodes::Class::cast(tree, id)
                .and_then(|n| n.type_parameters(tree))
                .or_else(|| nodes::TypeAlias::cast(tree, id).and_then(|n| n.type_parameters(tree)))
                .or_else(|| {
                    method_header(tree, id)
                        .and_then(|n| nodes::MethodHeader::cast(tree, n)?.type_parameters(tree))
                });
            if let Some(param_list) = param_list
                && let Some(tp) = nodes::TypeParameterList::cast(tree, param_list)?
                    .parameters(tree)
                    .find(|&n| tree.name(n) == Some(name))
            {
                return Some(NodeRef::new(node.file, tp));
            }
        }
        current = tree.parent(id);
    }
    None
}

/// If `let_node`'s TypedIdentifier matches `name`, return its
/// `ResolvedType`; otherwise `None`.
fn match_let_binding(
    let_node: NodeRef,
    name: &str,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<ResolvedType> {
    let file = let_node.file;
    let tree = ctx.tree(file)?;
    let let_node = let_node.value;
    let expr = nodes::LetExpr::cast(tree, let_node)?;
    let typed_identifier = expr.binding(tree)?;
    if tree.name(typed_identifier) != Some(name) {
        return None;
    }

    // Prefer the explicit annotation, otherwise infer from the initializer.
    let ty_from_annotation = nodes::TypedIdentifier::cast(tree, typed_identifier)?
        .type_annotation(tree)
        .and_then(|ann| nodes::TypeAnnotation::cast(tree, ann)?.ty(tree))
        .map(|tn| to_type_with_bindings(NodeRef::new(file, tn), bindings, ctx));

    let ty = ty_from_annotation.unwrap_or_else(|| {
        expr.value(tree)
            .map(|expr| compute_expr_type(NodeRef::new(file, expr), bindings, ctx))
            .unwrap_or(Type::Unknown)
    });

    Some(ResolvedType::new(NodeRef::new(file, typed_identifier), ty))
}

/// If any `TypedIdentifier` child of `param_list` matches `name`, return its
/// `ResolvedType`.
fn match_param(
    param_list: NodeRef,
    name: &str,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<ResolvedType> {
    let file = param_list.file;
    let tree = ctx.tree(file)?;
    let param_list = param_list.value;
    let typed_identifier = nodes::ParameterList::cast(tree, param_list)?
        .parameters(tree)
        .find(|&n| tree.name(n) == Some(name))?;

    let ty = nodes::TypedIdentifier::cast(tree, typed_identifier)?
        .type_annotation(tree)
        .and_then(|ann| nodes::TypeAnnotation::cast(tree, ann)?.ty(tree))
        .map(|tn| to_type_with_bindings(NodeRef::new(file, tn), bindings, ctx))
        .unwrap_or(Type::Unknown);
    Some(ResolvedType::new(NodeRef::new(file, typed_identifier), ty))
}

pub(crate) fn resolve_unqualified_access_with_bindings(
    node: NodeRef,
    name: &str,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<ResolvedType> {
    let file = node.file;
    let tree = ctx.tree(file)?;
    let node = node.value;
    if let Some(local) = resolve_local_binding(NodeRef::new(file, node), name, bindings, ctx) {
        return Some(local);
    };
    let receiver = compute_this_type(NodeRef::new(file, node), bindings, ctx);
    for ty in [
        &receiver,
        &module_type(NodeRef::new(file, tree.root()), ctx),
    ] {
        let (root, body) = match ty.unconstrained() {
            Type::Module { module_node, .. } => (*module_node, module_node.value),
            Type::Class { class_node, .. } => {
                let tree = ctx.tree(class_node.file)?;
                (
                    *class_node,
                    match nodes::Class::cast(tree, class_node.value).and_then(|n| n.body(tree)) {
                        Some(body) => body,
                        None => continue,
                    },
                )
            }
            _ => continue,
        };
        let tree = ctx.tree(root.file)?;
        let member = nodes::Module::cast(tree, body)
            .into_iter()
            .flat_map(|n| n.members(tree))
            .chain(
                nodes::ClassBody::cast(tree, body)
                    .into_iter()
                    .flat_map(|n| n.members(tree)),
            )
            .find(|&id| tree.name(id) == Some(name));
        if let Some(member) = member {
            let r = root.with_value(member);
            return Some(ResolvedType::new(
                r,
                definition_type_with_bindings(r, bindings, ctx),
            ));
        }
    }
    if let Some(import) = find_import(tree, name) {
        let declaration = NodeRef::new(file, import);
        let target = ctx
            .import(declaration)
            .and_then(|resolution| resolution.single())
            .unwrap_or(declaration);
        return Some(ResolvedType::new(target, import_type(declaration, ctx)));
    }
    if let Some(this) = find_member_on_type(
        &compute_this_type(NodeRef::new(file, node), bindings, ctx),
        name,
        bindings,
        ctx,
    ) {
        return Some(this);
    };
    if let Some(module_type) = find_member_on_type(
        &Type::Module {
            module_node: NodeRef::new(file, tree.root()),
        },
        name,
        bindings,
        ctx,
    ) {
        return Some(module_type);
    };

    if let Some(base_type) = resolve_type_name_in_module(ctx.tree(ctx.base.file_id())?, name) {
        return Some(ResolvedType::new(
            NodeRef::new(ctx.base.file_id(), base_type),
            to_type_with_bindings(NodeRef::new(ctx.base.file_id(), base_type), bindings, ctx),
        ));
    };

    None
}

pub(crate) fn class_member_bindings(
    class_node: NodeRef,
    type_args: &[Type],
    caller_bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> TypeBindings {
    let Some(tree) = ctx.tree(class_node.file) else {
        return caller_bindings.clone();
    };
    let mut member_bindings = caller_bindings.clone();
    if let Some(type_param_list) =
        nodes::Class::cast(tree, class_node.value).and_then(|n| n.type_parameters(tree))
    {
        let params = nodes::TypeParameterList::cast(tree, type_param_list)
            .into_iter()
            .flat_map(|n| n.parameters(tree));
        for (param_node, arg) in params.zip(type_args.iter()) {
            member_bindings.insert(NodeRef::new(class_node.file, param_node), arg.clone());
        }
    };
    member_bindings
}

/// Compute the type of the receiver of an access expression.
pub(crate) fn access_receiver_type_with_bindings(
    access: NodeRef,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<Type> {
    let file = access.file;
    let tree = ctx.tree(file)?;
    let access = access.value;
    match tree.kind(access) {
        NodeKind::UnqualifiedAccessExpr => {
            Some(compute_this_type(NodeRef::new(file, access), bindings, ctx))
        }
        NodeKind::QualifiedAccessExpr => {
            let receiver = nodes::QualifiedAccessExpr::cast(tree, access)?.receiver(tree)?;
            Some(compute_expr_type(
                NodeRef::new(file, receiver),
                bindings,
                ctx,
            ))
        }
        NodeKind::SuperAccessExpr => super_type(
            &compute_this_type(NodeRef::new(file, access), bindings, ctx),
            ctx,
        ),
        _ => None,
    }
}

/// Return the key type of a container-shaped class type at this position.
///
/// - `Listing<E>` / `List<E>` → `Int` (indices)
/// - `Mapping<K,V>` / `Map<K,V>` → `K`
/// - Anything else → `None`
pub fn container_key_type(ty: &Type, ctx: &AnalysisContext) -> Option<Type> {
    let ty = ty.unaliased(ctx);
    let Type::Class { type_args, .. } = ty.unconstrained() else {
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
    let ty = ty.unaliased(ctx);
    let Type::Class { type_args, .. } = ty.unconstrained() else {
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

/// Resolve a declaration without exposing internal generic bindings.
pub fn definition_type(node: NodeRef, ctx: &AnalysisContext) -> Type {
    definition_type_with_bindings(node, &TypeBindings::new(), ctx)
}

/// Resolve an unqualified access in its pinned file.
pub fn resolve_unqualified_access(
    node: NodeRef,
    name: &str,
    ctx: &AnalysisContext,
) -> Option<ResolvedType> {
    resolve_unqualified_access_with_bindings(node, name, &TypeBindings::new(), ctx)
}

/// Resolve an access receiver in its pinned file.
pub fn access_receiver_type(node: NodeRef, ctx: &AnalysisContext) -> Option<Type> {
    access_receiver_type_with_bindings(node, &TypeBindings::new(), ctx)
}
