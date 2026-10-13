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
    if let Some(name) = tree.name(import) {
        return Some(name.to_owned());
    }
    let uri = nodes::Import::cast(tree, import)
        .and_then(|n| n.uri(tree))
        .or_else(|| nodes::ImportGlob::cast(tree, import).and_then(|n| n.uri(tree)))?;
    let uri = tree.escaped_string_literal(uri)?;
    Some(module_uri_name(&uri).to_owned())
}

pub(crate) fn module_uri_name(uri: &str) -> &str {
    let name = uri
        .rsplit('/')
        .next()
        .unwrap_or(uri)
        .rsplit(':')
        .next()
        .unwrap_or(uri)
        .split(['?', '#'])
        .next()
        .unwrap_or(uri);
    name.strip_suffix(".pkl").unwrap_or(name)
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

/// Look up a declaration through the shared position-independent member tables.
pub(crate) fn find_member_on_type(
    ty: &Type,
    name: &str,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<ResolvedType> {
    match ty.unconstrained() {
        Type::Class {
            class_node,
            type_args,
        } => {
            let class = crate::class_id(ctx.db, *class_node, ctx.root)?;
            let entry = crate::class_members(ctx.db, class)
                .entries
                .iter()
                .find(|entry| entry.name.text(ctx.db) == name)?;
            let node = crate::item_node(ctx.db, entry.declaration)?;
            let ty = if type_args.is_empty() {
                definition_type_with_bindings(node, bindings, ctx)
            } else {
                let member_bindings = class_member_bindings(*class_node, type_args, bindings, ctx);
                definition_type_with_bindings(node, &member_bindings, ctx)
            };
            Some(ResolvedType::new(node, ty))
        }
        Type::Module { module_node } => {
            let module = crate::ModuleId::new(ctx.db, module_node.file, ctx.root);
            let entry = crate::module_members(ctx.db, module)
                .entries
                .iter()
                .find(|entry| entry.name.text(ctx.db) == name)?;
            let node = crate::item_node(ctx.db, entry.declaration)?;
            Some(ResolvedType::new(
                node,
                definition_type_with_bindings(node, bindings, ctx),
            ))
        }
        Type::Alias { .. } => find_member_on_type(&ty.unaliased(ctx), name, bindings, ctx),
        Type::Union { members } => {
            let mut node = None;
            let mut types = Vec::new();
            for alternative in members.iter() {
                if let Some(resolved) = find_member_on_type(alternative, name, bindings, ctx) {
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

pub(crate) fn resolve_unqualified_access_with_bindings(
    node: NodeRef,
    name: &str,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Option<ResolvedType> {
    let scope = crate::scope_at(node, ctx);
    let entry = scope.lookup(name)?;
    Some(ResolvedType::new(
        entry.node(ctx)?,
        entry.ty_with_bindings(bindings, ctx),
    ))
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
