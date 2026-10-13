//! Member tables depend exclusively on lowered declarations and URI routing.
use crate::{MemberKind, NodeRef, items::*};
use la_arena::Idx;
use pkl_db::{Db, File, ImportResolution};
use std::{collections::HashSet, sync::Arc};

#[salsa::interned(unsafe(no_lifetime), revisions = usize::MAX, debug)]
pub struct ModuleId {
    #[returns(copy)]
    pub file: File,
    #[returns(copy)]
    pub root: File,
}
#[salsa::interned(unsafe(no_lifetime), revisions = usize::MAX, debug)]
pub struct ClassId {
    #[returns(copy)]
    pub module: ModuleId,
    #[returns(copy)]
    pub index: Idx<ClassDecl>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberDecl {
    pub name: Name,
    pub declaration: DeclId,
    pub kind: MemberKind,
    pub modifiers: Modifiers,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemberTable {
    pub entries: Vec<MemberDecl>,
    pub own_len: usize,
}
pub fn class_id(db: &dyn Db, node: NodeRef, root: File) -> Option<ClassId> {
    let declaration = decl_for_node(db, node)?;
    let ItemId::Class(index) = declaration.item else {
        return None;
    };
    Some(ClassId::new(db, ModuleId::new(db, node.file, root), index))
}
pub(crate) fn base_module(db: &dyn Db, root: File) -> Option<ModuleId> {
    static URI: std::sync::LazyLock<fluent_uri::Uri<String>> =
        std::sync::LazyLock::new(|| fluent_uri::Uri::parse("pkl:base".to_owned()).unwrap());
    db.file(&URI).ok().map(|file| ModuleId::new(db, file, root))
}
fn parent_module(db: &dyn Db, module: ModuleId) -> Option<ModuleId> {
    let file = module.file(db);
    let uri = module_items(db, file).extends.as_ref()?;
    let project = pkl_db::contextual_project(db, module.root(db), file);
    match pkl_db::resolve_module_uri(db, file, uri.clone(), false, project) {
        ImportResolution::Single(file) => Some(ModuleId::new(db, file, module.root(db))),
        _ => None,
    }
}
fn push(
    db: &dyn Db,
    file: File,
    item: ItemId,
    table: &mut MemberTable,
    names: &mut HashSet<Name>,
    inherited: bool,
) {
    let Some((declaration, kind)) = module_items(db, file).declaration(item) else {
        return;
    };
    if inherited && declaration.modifiers.contains(Modifiers::LOCAL) {
        return;
    }
    if names.insert(declaration.name) {
        table.entries.push(MemberDecl {
            name: declaration.name,
            declaration: DeclId { file, item },
            kind,
            modifiers: declaration.modifiers,
        });
    }
}
// Resolve class paths without touching source maps. Aliases and module inheritance
// share the same visited set, so malformed cyclic declarations remain finite.
fn resolve_class(
    db: &dyn Db,
    module: ModuleId,
    ty: &TypeRef,
    visited: &mut HashSet<DeclId>,
) -> Option<ClassId> {
    match ty {
        TypeRef::Parenthesized(inner)
        | TypeRef::Nullable(inner)
        | TypeRef::Default(inner)
        | TypeRef::Constrained { base: inner, .. } => {
            return resolve_class(db, module, inner, visited);
        }
        _ => {}
    }
    let TypeRef::Named { path, .. } = ty else {
        return None;
    };
    resolve_class_path(db, module, path, visited, true)
}

fn resolve_class_path(
    db: &dyn Db,
    module: ModuleId,
    path: &[Name],
    visited: &mut HashSet<DeclId>,
    allow_base: bool,
) -> Option<ClassId> {
    let (&first, tail) = path.split_first()?;
    if !tail.is_empty() {
        let items = module_items(db, module.file(db));
        let import = items.imports.iter().find(|(_, i)| i.name == first)?.1;
        if import.glob {
            return None;
        }
        let context = pkl_db::contextual_project(db, module.root(db), module.file(db));
        let ImportResolution::Single(file) =
            pkl_db::resolve_module_uri(db, module.file(db), import.uri.clone(), false, context)
        else {
            return None;
        };
        // Qualification selects an exported type, not its lexical base namespace.
        return resolve_class_path(
            db,
            ModuleId::new(db, file, module.root(db)),
            tail,
            visited,
            false,
        );
    }
    let mut current = Some(module);
    let mut modules = HashSet::new();
    while let Some(m) = current {
        if !modules.insert(m) {
            break;
        }
        let items = module_items(db, m.file(db));
        if let Some((index, _)) = items
            .classes
            .iter()
            .find(|(_, c)| c.declaration.name == first)
        {
            return Some(ClassId::new(db, m, index));
        }
        if let Some((index, alias)) = items
            .type_aliases
            .iter()
            .find(|(_, a)| a.declaration.name == first)
        {
            if !visited.insert(DeclId {
                file: m.file(db),
                item: ItemId::TypeAlias(index),
            }) {
                return None;
            }
            return resolve_class(db, m, &alias.ty, visited);
        }
        current = parent_module(db, m);
    }
    if !allow_base {
        return None;
    }
    let base = base_module(db, module.root(db))?;
    if base == module {
        return None;
    }
    let items = module_items(db, base.file(db));
    if let Some((index, _)) = items
        .classes
        .iter()
        .find(|(_, c)| c.declaration.name == first)
    {
        return Some(ClassId::new(db, base, index));
    }
    if let Some((index, alias)) = items
        .type_aliases
        .iter()
        .find(|(_, a)| a.declaration.name == first)
    {
        if !visited.insert(DeclId {
            file: base.file(db),
            item: ItemId::TypeAlias(index),
        }) {
            return None;
        }
        return resolve_class(db, base, &alias.ty, visited);
    }
    None
}
fn collect_class(
    db: &dyn Db,
    class: ClassId,
    table: &mut MemberTable,
    names: &mut HashSet<Name>,
    visited: &mut HashSet<ClassId>,
    inherited: bool,
) {
    if !visited.insert(class) {
        return;
    }
    let module = class.module(db);
    let file = module.file(db);
    let items = module_items(db, file);
    let Some(declaration) = arena_item(&items.classes, class.index(db)) else {
        return;
    };
    for &item in &declaration.members {
        push(db, file, item, table, names, inherited);
    }
    if !inherited {
        table.own_len = table.entries.len();
    }
    let parent = match &declaration.extends {
        Some(ty) => resolve_class(db, module, ty, &mut HashSet::new()),
        // Native SDK classes inherit Any even when the SDK source omits the clause.
        None if declaration
            .declaration
            .modifiers
            .contains(Modifiers::EXTERNAL) =>
        {
            base_module(db, module.root(db)).and_then(|base| {
                if file != base.file(db) || declaration.declaration.name.text(db) == "Any" {
                    return None;
                }
                module_items(db, file)
                    .classes
                    .iter()
                    .find(|(_, c)| c.declaration.name.text(db) == "Any")
                    .map(|(index, _)| ClassId::new(db, base, index))
            })
        }
        None => None,
    };
    if let Some(parent) = parent {
        collect_class(db, parent, table, names, visited, true);
    }
}
#[salsa::tracked(returns(ref))]
pub fn class_members(db: &dyn Db, class: ClassId) -> Arc<MemberTable> {
    let mut table = MemberTable::default();
    collect_class(
        db,
        class,
        &mut table,
        &mut HashSet::new(),
        &mut HashSet::new(),
        false,
    );
    Arc::new(table)
}
#[salsa::tracked(returns(ref))]
pub fn module_members(db: &dyn Db, module: ModuleId) -> Arc<MemberTable> {
    let mut table = MemberTable::default();
    let mut names = HashSet::new();
    let mut visited = HashSet::new();
    let mut current = Some(module);
    let mut own = true;
    while let Some(m) = current {
        if !visited.insert(m) {
            break;
        }
        for &item in &module_items(db, m.file(db)).declarations {
            push(db, m.file(db), item, &mut table, &mut names, !own);
        }
        if own {
            table.own_len = table.entries.len();
            own = false;
        }
        current = parent_module(db, m);
    }
    // Module instances inherit the SDK Module class, not the declarations of pkl:base.
    if let Some(base) = base_module(db, module.root(db)) {
        let items = module_items(db, base.file(db));
        if let Some((index, _)) = items
            .classes
            .iter()
            .find(|(_, c)| c.declaration.name.text(db) == "Module")
        {
            collect_class(
                db,
                ClassId::new(db, base, index),
                &mut table,
                &mut names,
                &mut HashSet::new(),
                true,
            );
        }
    }
    Arc::new(table)
}
