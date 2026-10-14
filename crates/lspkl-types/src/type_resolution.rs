//! Position-independent type resolution over lowered declaration headers.
use crate::{ConstraintExpr, Type, items::*, member_queries::*, ty::TypeBindings};
use lspkl_db::{Db, File, ImportResolution};
use std::collections::HashSet;

#[salsa::interned(unsafe(no_lifetime), revisions = usize::MAX)]
struct ResolutionRequest {
    #[returns(copy)]
    file: File,
    #[returns(copy)]
    root: File,
    #[returns(copy)]
    owner: Option<DeclId>,
    #[returns(ref)]
    ty: TypeRef,
    #[returns(ref)]
    bindings: TypeBindings,
}
fn cycle_unknown(_: &dyn Db, _: salsa::Id, _: ResolutionRequest) -> Type {
    Type::Unknown
}
fn alias_cycle_unknown(_: &dyn Db, _: salsa::Id, _: AliasTemplateRequest) -> Type {
    Type::Unknown
}
#[salsa::tracked(returns(clone), cycle_result = cycle_unknown)]
fn resolve_request(db: &dyn Db, request: ResolutionRequest) -> Type {
    if request.file(db).status(db) == lspkl_db::FileStatus::Missing {
        return Type::Unknown;
    }
    let bindings = request.bindings(db);
    resolve_inner(
        db,
        request.file(db),
        request.root(db),
        request.owner(db),
        request.ty(db),
        bindings,
    )
}
/// Resolve a lowered type reference without caller-owned substitutions.
pub fn resolve_type_ref(db: &dyn Db, owner: DeclId, ty: &TypeRef) -> Type {
    resolve_type_ref_with_bindings(db, owner, ty, &TypeBindings::new())
}

pub(crate) fn resolve_type_ref_with_bindings(
    db: &dyn Db,
    owner: DeclId,
    ty: &TypeRef,
    bindings: &TypeBindings,
) -> Type {
    resolve_in_file(db, owner.file, owner.root, Some(owner), ty, bindings)
}
pub(crate) fn resolve_in_file(
    db: &dyn Db,
    file: File,
    root: File,
    owner: Option<DeclId>,
    ty: &TypeRef,
    bindings: &TypeBindings,
) -> Type {
    resolve_request(
        db,
        ResolutionRequest::new(db, file, root, owner, ty, bindings),
    )
}
pub(crate) fn parameters(db: &dyn Db, owner: DeclId) -> &[TypeParameter] {
    let items = module_items(db, owner.file);
    match owner.item {
        ItemId::Class(i) => {
            arena_item(&items.classes, i).map_or(&[], |decl| decl.type_parameters.as_slice())
        }
        ItemId::TypeAlias(i) => {
            arena_item(&items.type_aliases, i).map_or(&[], |decl| decl.type_parameters.as_slice())
        }
        ItemId::Method(i) => {
            arena_item(&items.methods, i).map_or(&[], |decl| decl.type_parameters.as_slice())
        }
        _ => &[],
    }
}
pub(crate) fn enclosing_class(db: &dyn Db, owner: DeclId) -> Option<DeclId> {
    module_items(db, owner.file)
        .classes
        .iter()
        .find(|(_, class)| class.members.contains(&owner.item))
        .map(|(index, _)| DeclId {
            item: ItemId::Class(index),
            ..owner
        })
}
pub(crate) fn type_parameter(db: &dyn Db, owner: DeclId, name: Name) -> Option<TypeParameterId> {
    parameters(db, owner)
        .iter()
        .position(|p| p.name == name)
        .map(|index| TypeParameterId {
            declaration: owner,
            index: index as u32,
        })
        .or_else(|| enclosing_class(db, owner).and_then(|class| type_parameter(db, class, name)))
}
pub(crate) fn lookup_type(
    db: &dyn Db,
    module: ModuleId,
    path: &[Name],
    lexical: bool,
) -> Option<DeclId> {
    let (&name, tail) = path.split_first()?;
    let mut current = Some(module);
    let mut seen = HashSet::new();
    while let Some(m) = current {
        if !seen.insert(m) {
            break;
        }
        let items = module_items(db, m.file(db));
        // A non-glob import names its module's type (`import "x.pkl" as X; p: X`).
        // Imports are lexical: only the referencing module's own imports apply,
        // never a parent's or a qualified target's. Like Pkl, they are searched
        // before the module's own classes and aliases.
        if tail.is_empty()
            && lexical
            && m == module
            && let Some((index, _)) = items
                .imports
                .iter()
                .find(|(_, import)| import.name == name && !import.glob)
        {
            return Some(DeclId {
                file: m.file(db),
                root: m.root(db),
                item: ItemId::Import(index),
            });
        }
        if tail.is_empty()
            && let Some(&item) = items.declarations.iter().find(|&&item| {
                matches!(item, ItemId::Class(_) | ItemId::TypeAlias(_))
                    && items.declaration(item).is_some_and(|(d, _)| d.name == name)
            })
        {
            return Some(DeclId {
                file: m.file(db),
                root: m.root(db),
                item,
            });
        }
        if !tail.is_empty()
            && let Some((_, import)) = items.imports.iter().find(|(_, import)| import.name == name)
        {
            if import.glob {
                return None;
            }
            let project = lspkl_db::contextual_project(db, m.root(db), m.file(db));
            let ImportResolution::Single(file) =
                lspkl_db::resolve_module_uri(db, m.file(db), import.uri.clone(), false, project)
            else {
                return None;
            };
            return lookup_type(db, module_id(db, file, m.root(db)), tail, false);
        }
        current = parent_module(db, m);
    }
    if lexical {
        let base = base_module(db, module.root(db))?;
        if base != module {
            return lookup_type(db, base, path, false);
        }
    }
    None
}
fn resolve_inner(
    db: &dyn Db,
    file: File,
    root: File,
    owner: Option<DeclId>,
    ty: &TypeRef,
    bindings: &TypeBindings,
) -> Type {
    let resolve = |ty: &TypeRef| resolve_in_file(db, file, root, owner, ty, bindings);
    match ty {
        TypeRef::Unknown => Type::Unknown,
        TypeRef::Nothing => Type::Nothing,
        TypeRef::Module => crate::base_module(db).module_type().clone(),
        TypeRef::StringLiteral(value) => Type::string_literal(value.clone()),
        TypeRef::Parenthesized(inner) | TypeRef::Default(inner) => resolve(inner),
        TypeRef::Nullable(inner) => resolve(inner).nullable(db),
        TypeRef::Union(left, right) => Type::union([resolve(left), resolve(right)], db),
        TypeRef::Constrained { base, constraints } => Type::constrained(
            resolve(base),
            constraints.iter().map(ConstraintExpr::opaque).collect(),
        ),
        TypeRef::Function { parameters, result } => {
            let base = crate::base_module(db);
            let mut args: Vec<_> = parameters.iter().map(resolve).collect();
            args.push(resolve(result));
            base.function_n_type(parameters.len())
                .unwrap_or(base.function_type().clone())
                .with_type_args(args)
        }
        TypeRef::Named { path, arguments } => {
            if path.len() == 1
                && let Some(&first) = path.first()
                && let Some(parameter) = owner.and_then(|owner| type_parameter(db, owner, first))
            {
                return bindings.get(&parameter).cloned().unwrap_or(Type::Unknown);
            }
            let Some(decl) = lookup_type(db, module_id(db, file, root), path, true) else {
                return Type::Unknown;
            };
            let args = arguments.iter().map(resolve).collect();
            match decl.item {
                ItemId::Class(index) => Type::class_with_args(
                    ClassId::new(db, module_id(db, decl.file, decl.root), index),
                    args,
                ),
                ItemId::TypeAlias(_) => Type::Alias {
                    alias_id: decl,
                    type_args: args,
                },
                ItemId::Import(_) => import_type(db, decl),
                _ => Type::Unknown,
            }
        }
    }
}
pub(crate) fn import_type(db: &dyn Db, decl: DeclId) -> Type {
    let ItemId::Import(index) = decl.item else {
        return Type::Unknown;
    };
    let Some(import) = arena_item(&module_items(db, decl.file).imports, index) else {
        return Type::Unknown;
    };
    let project = lspkl_db::contextual_project(db, decl.root, decl.file);
    match lspkl_db::resolve_module_uri(db, decl.file, import.uri.clone(), import.glob, project) {
        ImportResolution::Single(file) => Type::Module {
            module_id: module_id(db, file, decl.root),
        },
        ImportResolution::Glob(files) => crate::base_module(db)
            .mapping_type()
            .clone()
            .with_type_args(vec![
                crate::base_module(db).string_type().clone(),
                Type::union(
                    files.iter().map(|&file| Type::Module {
                        module_id: module_id(db, file, decl.root),
                    }),
                    db,
                ),
            ]),
        _ => Type::Unknown,
    }
}
#[salsa::interned(unsafe(no_lifetime), revisions = usize::MAX)]
struct AliasTemplateRequest {
    #[returns(copy)]
    alias: DeclId,
}
// Resolve each alias body once with symbolic parameters. The declaration-only key
// detects recursive bodies even when recursion grows the generic argument shape.
#[salsa::tracked(returns(clone), cycle_result = alias_cycle_unknown)]
fn alias_template(db: &dyn Db, request: AliasTemplateRequest) -> Type {
    let alias = request.alias(db);
    let ItemId::TypeAlias(index) = alias.item else {
        return Type::Unknown;
    };
    let Some(declaration) = arena_item(&module_items(db, alias.file).type_aliases, index) else {
        return Type::Unknown;
    };
    let bindings = parameters(db, alias)
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let parameter = TypeParameterId {
                declaration: alias,
                index: index as u32,
            };
            (
                parameter,
                Type::Variable {
                    type_param: parameter,
                },
            )
        })
        .collect();
    resolve_type_ref_with_bindings(db, alias, &declaration.ty, &bindings).unaliased(db)
}
fn substitute(db: &dyn Db, ty: Type, bindings: &TypeBindings) -> Type {
    match ty {
        Type::Variable { type_param } => bindings
            .get(&type_param)
            .cloned()
            .unwrap_or(Type::Variable { type_param }),
        Type::Class {
            class_id,
            type_args,
        } => Type::class_with_args(
            class_id,
            type_args
                .into_iter()
                .map(|ty| substitute(db, ty, bindings))
                .collect(),
        ),
        Type::Alias {
            alias_id,
            type_args,
        } => Type::Alias {
            alias_id,
            type_args: type_args
                .into_iter()
                .map(|ty| substitute(db, ty, bindings))
                .collect(),
        },
        Type::Union { members } => Type::union(
            members.into_iter().map(|ty| substitute(db, ty, bindings)),
            db,
        ),
        Type::Constrained { base, constraints } => {
            Type::constrained(substitute(db, *base, bindings), constraints)
        }
        ty => ty,
    }
}
pub fn resolve_alias(db: &dyn Db, alias: DeclId, args: &[Type]) -> Type {
    let template = alias_template(db, AliasTemplateRequest::new(db, alias));
    let bindings = parameters(db, alias)
        .iter()
        .enumerate()
        .map(|(index, _)| {
            (
                TypeParameterId {
                    declaration: alias,
                    index: index as u32,
                },
                args.get(index).cloned().unwrap_or(Type::Unknown),
            )
        })
        .collect();
    substitute(db, template, &bindings).unaliased(db)
}
