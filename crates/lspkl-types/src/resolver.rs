//! Name resolution and class hierarchy queries.

use lspkl_syntax::TextSize;
use lspkl_syntax::ast::nodes;
use lspkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};

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

/// Look up the definition of a declared type
pub fn resolve_declared_type(ctx: &AnalysisContext, node: NodeRef) -> Option<NodeRef> {
    let tree = ctx.tree(node.file)?;
    let name_parts = tree.declared_type_parts(node.value);
    resolve_type_name(ctx, &name_parts, node)
}

pub fn resolve_type_name(ctx: &AnalysisContext, parts: &[&str], node: NodeRef) -> Option<NodeRef> {
    let path: Vec<_> = parts
        .iter()
        .map(|part| crate::Name::new(ctx.db, (*part).to_owned()))
        .collect();
    if path.len() == 1
        && let Some(&name) = path.first()
        && let Some(owner) = crate::to_type::owner_for_node(ctx.db, node, ctx.root)
        && let Some(parameter) = crate::type_resolution::type_parameter(ctx.db, owner, name)
    {
        let declaration = crate::item_node(ctx.db, parameter.declaration)?;
        let tree = ctx.tree(declaration.file)?;
        let list = nodes::Class::cast(tree, declaration.value)
            .and_then(|n| n.type_parameters(tree))
            .or_else(|| {
                nodes::TypeAlias::cast(tree, declaration.value)
                    .and_then(|n| n.type_parameters(tree))
            })
            .or_else(|| {
                method_header(tree, declaration.value)
                    .and_then(|n| nodes::MethodHeader::cast(tree, n)?.type_parameters(tree))
            })?;
        return nodes::TypeParameterList::cast(tree, list)?
            .parameters(tree)
            .nth(parameter.index as usize)
            .map(|id| declaration.with_value(id));
    }
    let decl = crate::type_resolution::lookup_type(
        ctx.db,
        crate::module_id(ctx.db, node.file, ctx.root),
        &path,
        true,
    )?;
    let declaration = crate::item_node(ctx.db, decl)?;
    if matches!(decl.item, crate::items::ItemId::Import(_)) {
        // An imported module's type navigates into that module; an unresolved
        // import stays at its declaration, which already reports the failure.
        return Some(
            ctx.import(declaration)
                .and_then(|resolution| resolution.single())
                .unwrap_or(declaration),
        );
    }
    Some(declaration)
}

pub fn module_type(module_node: NodeRef, ctx: &AnalysisContext) -> Type {
    Type::Module {
        module_id: crate::module_id(ctx.db, module_node.file, ctx.root),
    }
}

pub fn module_super(module: crate::ModuleId, db: &dyn lspkl_db::Db) -> Option<crate::ModuleId> {
    crate::member_queries::parent_module(db, module)
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
            let value = Type::union(roots.iter().map(|r| module_type(*r, ctx)), ctx.db);
            ctx.base
                .mapping_type()
                .clone()
                .with_type_args(vec![ctx.base.string_type().clone(), value])
        }
        None | Some(crate::ImportResolution::Unresolved { .. }) => Type::Unknown,
    }
}

pub fn is_subclass(mut a: crate::ClassId, b: crate::ClassId, db: &dyn lspkl_db::Db) -> bool {
    let mut seen = std::collections::HashSet::new();
    while seen.insert(a) {
        if a == b {
            return true;
        }
        let Some(Type::Class { class_id, .. }) = super_type(&Type::class(a), db) else {
            return false;
        };
        a = class_id;
    }
    false
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
        NodeKind::ClassProperty | NodeKind::ObjectProperty => {
            let name = tree.identifier(node_id)?;
            tree.span(name)
                .contains_inclusive(offset)
                .then_some(node.with_value(node_id))
        }
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
        Type::Class { class_id, .. } => {
            let entry = crate::class_members(ctx.db, *class_id)
                .entries
                .iter()
                .find(|e| e.name.text(ctx.db) == name)?;
            let bindings = class_member_bindings(ty, bindings, ctx);
            Some(ResolvedType::new(
                crate::item_node(ctx.db, entry.declaration)?,
                declaration_type_with_bindings(ctx.db, entry.declaration, &bindings),
            ))
        }
        Type::Module { module_id } => {
            let entry = crate::module_members(ctx.db, *module_id)
                .entries
                .iter()
                .find(|e| e.name.text(ctx.db) == name)?;
            Some(ResolvedType::new(
                crate::item_node(ctx.db, entry.declaration)?,
                declaration_type_with_bindings(ctx.db, entry.declaration, bindings),
            ))
        }
        Type::Alias { .. } => find_member_on_type(&ty.unaliased(ctx.db), name, bindings, ctx),
        Type::Union { members } => {
            let mut node = None;
            let types = members
                .iter()
                .filter_map(|ty| find_member_on_type(ty, name, bindings, ctx))
                .map(|resolved| {
                    node.get_or_insert(resolved.node);
                    resolved.ty
                })
                .collect::<Vec<_>>();
            node.map(|node| ResolvedType::new(node, Type::union(types, ctx.db)))
        }
        _ => None,
    }
}
pub(crate) fn declaration_type_with_bindings(
    db: &dyn lspkl_db::Db,
    decl: crate::DeclId,
    bindings: &TypeBindings,
) -> Type {
    use crate::items::*;
    let items = module_items(db, decl.file);
    let annotation = match decl.item {
        ItemId::Property(i) => arena_item(&items.properties, i).and_then(|decl| decl.ty.as_ref()),
        ItemId::Method(i) => arena_item(&items.methods, i).and_then(|decl| decl.result.as_ref()),
        _ => None,
    };
    if let Some(annotation) = annotation {
        return crate::type_resolution::resolve_type_ref_with_bindings(
            db, decl, annotation, bindings,
        );
    }
    if !bindings.is_empty() && matches!(decl.item, ItemId::Property(_) | ItemId::Method(_)) {
        let Some(node) = crate::item_node(db, decl) else {
            return Type::Unknown;
        };
        let ctx = AnalysisContext::for_root(db, decl.root);
        let Some(tree) = ctx.tree(decl.file) else {
            return Type::Unknown;
        };
        return member_value(tree, node.value)
            .map(|expr| compute_expr_type(node.with_value(expr), bindings, &ctx))
            .unwrap_or(Type::Unknown);
    }
    crate::inference::decl_type(db, decl)
}
pub(crate) fn compute_decl_type(db: &dyn lspkl_db::Db, decl: crate::DeclId) -> Type {
    use crate::items::*;
    let items = module_items(db, decl.file);
    if !matches!(decl.item, ItemId::Import(_)) && items.declaration(decl.item).is_none() {
        return Type::Unknown;
    }
    match decl.item {
        ItemId::Class(index) => {
            return Type::class(crate::ClassId::new(
                db,
                crate::module_id(db, decl.file, decl.root),
                index,
            ));
        }
        ItemId::TypeAlias(_) => {
            return Type::Alias {
                alias_id: decl,
                type_args: vec![],
            };
        }
        ItemId::Import(_) => return crate::type_resolution::import_type(db, decl),
        ItemId::Property(i) => {
            if let Some(ty) = arena_item(&items.properties, i).and_then(|decl| decl.ty.as_ref()) {
                return crate::type_resolution::resolve_type_ref_with_bindings(
                    db,
                    decl,
                    ty,
                    &TypeBindings::new(),
                );
            }
        }
        ItemId::Method(i) => {
            if let Some(ty) = arena_item(&items.methods, i).and_then(|decl| decl.result.as_ref()) {
                return crate::type_resolution::resolve_type_ref_with_bindings(
                    db,
                    decl,
                    ty,
                    &TypeBindings::new(),
                );
            }
        }
    }
    let Some(node) = crate::item_node(db, decl) else {
        return Type::Unknown;
    };
    let ctx = AnalysisContext::for_root(db, decl.root);
    let Some(tree) = ctx.tree(decl.file) else {
        return Type::Unknown;
    };
    member_value(tree, node.value)
        .map(|expr| compute_expr_type(node.with_value(expr), &TypeBindings::new(), &ctx))
        .unwrap_or(Type::Unknown)
}
pub(crate) fn definition_type_with_bindings(
    node: NodeRef,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Type {
    if let Some(decl) = crate::decl_for_node(ctx.db, node, ctx.root) {
        return declaration_type_with_bindings(ctx.db, decl, bindings);
    }
    let Some(tree) = ctx.tree(node.file) else {
        return Type::Unknown;
    };
    if let Some(annotation) = member_type_annotation(tree, node.value)
        .and_then(|id| nodes::TypeAnnotation::cast(tree, id)?.ty(tree))
    {
        return to_type_with_bindings(node.with_value(annotation), bindings, ctx);
    }
    member_value(tree, node.value)
        .map(|expr| compute_expr_type(node.with_value(expr), bindings, ctx))
        .unwrap_or(Type::Unknown)
}
pub fn super_type(ty: &Type, db: &dyn lspkl_db::Db) -> Option<Type> {
    use crate::items::*;
    match ty.unconstrained() {
        Type::Module { module_id } => crate::member_queries::parent_module(db, *module_id)
            .map(|module_id| Type::Module { module_id }),
        Type::Class {
            class_id,
            type_args,
        } => {
            let module = class_id.module(db);
            let decl = DeclId {
                file: module.file(db),
                root: module.root(db),
                item: ItemId::Class(class_id.index(db)),
            };
            let class = arena_item(&module_items(db, decl.file).classes, class_id.index(db))?;
            if let Some(extends) = &class.extends {
                let bindings = class
                    .type_parameters
                    .iter()
                    .enumerate()
                    .map(|(index, _)| {
                        (
                            TypeParameterId {
                                declaration: decl,
                                index: index as u32,
                            },
                            type_args.get(index).cloned().unwrap_or(Type::Unknown),
                        )
                    })
                    .collect();
                Some(
                    crate::type_resolution::resolve_type_ref_with_bindings(
                        db, decl, extends, &bindings,
                    )
                    .unaliased(db),
                )
            } else if class.declaration.modifiers.contains(Modifiers::EXTERNAL)
                && class.declaration.name.text(db) != "Any"
            {
                Some(crate::base_module(db).any_type().clone())
            } else {
                None
            }
        }
        Type::Alias { .. } => super_type(&ty.unaliased(db), db),
        _ => None,
    }
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
    receiver: &Type,
    caller_bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> TypeBindings {
    let mut bindings = caller_bindings.clone();
    let mut current = std::borrow::Cow::Borrowed(receiver);
    let mut seen = std::collections::HashSet::new();
    while let Type::Class {
        class_id,
        type_args,
    } = current.unconstrained()
    {
        if !seen.insert(*class_id) {
            break;
        }
        let module = class_id.module(ctx.db);
        let decl = crate::DeclId {
            file: module.file(ctx.db),
            root: module.root(ctx.db),
            item: crate::ItemId::Class(class_id.index(ctx.db)),
        };
        for (index, arg) in type_args.iter().enumerate() {
            bindings.insert(
                crate::items::TypeParameterId {
                    declaration: decl,
                    index: index as u32,
                },
                arg.clone(),
            );
        }
        let Some(parent) = super_type(&current, ctx.db) else {
            break;
        };
        current = std::borrow::Cow::Owned(parent);
    }
    bindings
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
            ctx.db,
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
    let ty = ty.unaliased(ctx.db);
    let Type::Class { type_args, .. } = ty.unconstrained() else {
        return None;
    };
    if ty.is_subtype_of(ctx.base.mapping_type(), ctx.db)
        || ty.is_subtype_of(ctx.base.map_type(), ctx.db)
    {
        type_args.first().cloned()
    } else if ty.is_subtype_of(ctx.base.listing_type(), ctx.db)
        || ty.is_subtype_of(ctx.base.list_type(), ctx.db)
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
    let ty = ty.unaliased(ctx.db);
    let Type::Class { type_args, .. } = ty.unconstrained() else {
        return None;
    };
    if ty.is_subtype_of(ctx.base.mapping_type(), ctx.db)
        || ty.is_subtype_of(ctx.base.map_type(), ctx.db)
    {
        type_args.get(1).cloned()
    } else if ty.is_subtype_of(ctx.base.listing_type(), ctx.db)
        || ty.is_subtype_of(ctx.base.list_type(), ctx.db)
        || ty.is_subtype_of(ctx.base.set_type(), ctx.db)
    {
        type_args.first().cloned()
    } else {
        None
    }
}

/// Resolve a declaration without exposing internal generic bindings.
pub fn definition_type(db: &dyn lspkl_db::Db, node: NodeRef, root: lspkl_db::File) -> Type {
    definition_type_with_bindings(
        node,
        &TypeBindings::new(),
        &AnalysisContext::for_root(db, root),
    )
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
