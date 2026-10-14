//! Member tables depend exclusively on lowered declarations and URI routing.
use crate::{MemberKind, NodeRef, items::*};
use la_arena::Idx;
use lspkl_db::{Db, File, ImportResolution};
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
pub fn module_id(db: &dyn Db, file: File, root: File) -> ModuleId {
    let root = if file.uri(db).scheme().as_str() == "pkl" {
        file
    } else {
        root
    };
    ModuleId::new(db, file, root)
}
impl PartialOrd for ModuleId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ModuleId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use salsa::plumbing::AsId;
        self.as_id().cmp(&other.as_id())
    }
}
impl PartialOrd for ClassId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ClassId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use salsa::plumbing::AsId;
        self.as_id().cmp(&other.as_id())
    }
}
pub fn class_id(db: &dyn Db, node: NodeRef, root: File) -> Option<ClassId> {
    let declaration = decl_for_node(db, node, root)?;
    let ItemId::Class(index) = declaration.item else {
        return None;
    };
    Some(ClassId::new(db, module_id(db, node.file, root), index))
}
pub(crate) fn base_module(db: &dyn Db, root: File) -> Option<ModuleId> {
    static URI: std::sync::LazyLock<fluent_uri::Uri<String>> =
        std::sync::LazyLock::new(|| fluent_uri::Uri::parse("pkl:base".to_owned()).unwrap());
    db.file(&URI).ok().map(|file| module_id(db, file, root))
}
pub(crate) fn parent_module(db: &dyn Db, module: ModuleId) -> Option<ModuleId> {
    let file = module.file(db);
    let uri = module_items(db, file).extends.as_ref()?;
    let project = lspkl_db::contextual_project(db, module.root(db), file);
    match lspkl_db::resolve_module_uri(db, file, uri.clone(), false, project) {
        ImportResolution::Single(file) => Some(module_id(db, file, module.root(db))),
        _ => None,
    }
}
fn push(
    db: &dyn Db,
    file: File,
    root: File,
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
            declaration: DeclId {
                file,
                item,
                root: module_id(db, file, root).root(db),
            },
            kind,
            modifiers: declaration.modifiers,
        });
    }
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
        push(db, file, module.root(db), item, table, names, inherited);
    }
    if !inherited {
        table.own_len = table.entries.len();
    }
    let parent =
        crate::resolver::super_type(&crate::Type::class(class), db).and_then(|ty| {
            match ty.unconstrained() {
                crate::Type::Class { class_id, .. } => Some(*class_id),
                _ => None,
            }
        });
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
            push(
                db,
                m.file(db),
                m.root(db),
                item,
                &mut table,
                &mut names,
                !own,
            );
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
