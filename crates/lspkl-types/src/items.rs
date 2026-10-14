//! Stable declarations, isolated from the syntax positions used to display them.
use crate::{MemberKind, NodeRef};
use la_arena::{Arena, Idx};
use lspkl_db::{Db, File};
use lspkl_syntax::ast::{NodeId, NodeKind, SyntaxTree, TokenType, nodes::*};
use std::{collections::HashMap, sync::Arc};

#[salsa::interned(unsafe(no_lifetime), revisions = usize::MAX, debug)]
pub struct Name {
    #[returns(ref)]
    pub text: String,
}
bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct Modifiers: u8 {
        const ABSTRACT = 1;
        const EXTERNAL = 2;
        const HIDDEN = 4;
        const LOCAL = 8;
        const OPEN = 16;
        const FIXED = 32;
        const CONST = 64;
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TypeRef {
    Unknown,
    Nothing,
    Module,
    StringLiteral(String),
    Named {
        path: Vec<Name>,
        arguments: Vec<TypeRef>,
    },
    Parenthesized(Box<TypeRef>),
    Nullable(Box<TypeRef>),
    Default(Box<TypeRef>),
    Union(Box<TypeRef>, Box<TypeRef>),
    Function {
        parameters: Vec<TypeRef>,
        result: Box<TypeRef>,
    },
    Constrained {
        base: Box<TypeRef>,
        constraints: Vec<String>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declaration {
    pub name: Name,
    pub modifiers: Modifiers,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeParameter {
    pub name: Name,
    pub variance: Variance,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassDecl {
    pub declaration: Declaration,
    pub type_parameters: Vec<TypeParameter>,
    pub extends: Option<TypeRef>,
    pub members: Vec<ItemId>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeAliasDecl {
    pub declaration: Declaration,
    pub type_parameters: Vec<TypeParameter>,
    pub ty: TypeRef,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyDecl {
    pub declaration: Declaration,
    pub ty: Option<TypeRef>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parameter {
    pub name: Option<Name>,
    pub ty: Option<TypeRef>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodDecl {
    pub declaration: Declaration,
    pub type_parameters: Vec<TypeParameter>,
    pub parameters: Vec<Parameter>,
    pub result: Option<TypeRef>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportDecl {
    pub name: Name,
    pub uri: String,
    pub glob: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ItemId {
    Class(Idx<ClassDecl>),
    TypeAlias(Idx<TypeAliasDecl>),
    Property(Idx<PropertyDecl>),
    Method(Idx<MethodDecl>),
    Import(Idx<ImportDecl>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeclId {
    pub file: File,
    pub item: ItemId,
    pub root: File,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeParameterId {
    pub declaration: DeclId,
    pub index: u32,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleItems {
    pub name: Option<Name>,
    pub extends: Option<String>,
    pub amends: bool,
    pub classes: Arena<ClassDecl>,
    pub type_aliases: Arena<TypeAliasDecl>,
    pub properties: Arena<PropertyDecl>,
    pub methods: Arena<MethodDecl>,
    pub imports: Arena<ImportDecl>,
    pub declarations: Vec<ItemId>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemSourceMap {
    pub nodes: HashMap<ItemId, NodeId>,
    pub declarations: HashMap<NodeId, ItemId>,
}

pub fn declaration_kind(tree: &SyntaxTree, node: NodeId) -> Option<MemberKind> {
    match tree.kind(node) {
        NodeKind::Class => Some(MemberKind::Class),
        NodeKind::TypeAlias => Some(MemberKind::TypeAlias),
        NodeKind::ClassProperty | NodeKind::ObjectProperty => Some(MemberKind::Property),
        NodeKind::ClassMethod | NodeKind::ObjectMethod => Some(MemberKind::Method),
        _ => None,
    }
}
fn modifiers(tree: &SyntaxTree, node: NodeId) -> Modifiers {
    tree.modifiers(node)
        .into_iter()
        .fold(Modifiers::empty(), |acc, token| {
            acc | match token {
                TokenType::Abstract => Modifiers::ABSTRACT,
                TokenType::External => Modifiers::EXTERNAL,
                TokenType::Hidden => Modifiers::HIDDEN,
                TokenType::Local => Modifiers::LOCAL,
                TokenType::Open => Modifiers::OPEN,
                TokenType::Fixed => Modifiers::FIXED,
                TokenType::Const => Modifiers::CONST,
                _ => Modifiers::empty(),
            }
        })
}
fn path(db: &dyn Db, tree: &SyntaxTree, node: NodeId) -> Vec<Name> {
    QualifiedIdentifier::cast(tree, node)
        .into_iter()
        .flat_map(|q| q.identifiers(tree))
        .map(|id| Name::new(db, lspkl_syntax::ast::unquote_identifier(tree.text(id))))
        .collect()
}
pub(crate) fn lower_type(db: &dyn Db, tree: &SyntaxTree, node: NodeId) -> TypeRef {
    let inner = |id: Option<NodeId>| {
        Box::new(
            id.map(|id| lower_type(db, tree, id))
                .unwrap_or(TypeRef::Unknown),
        )
    };
    match tree.kind(node) {
        NodeKind::NothingType => TypeRef::Nothing,
        NodeKind::ModuleType => TypeRef::Module,
        NodeKind::StringLiteralType => TypeRef::StringLiteral(
            tree.escaped_string_literal(node)
                .unwrap_or_else(|| tree.text(node).to_owned()),
        ),
        NodeKind::DeclaredType => {
            let ty = DeclaredType::cast(tree, node).unwrap();
            TypeRef::Named {
                path: ty.name(tree).map(|n| path(db, tree, n)).unwrap_or_default(),
                arguments: ty
                    .arguments(tree)
                    .map(|n| lower_type(db, tree, n))
                    .collect(),
            }
        }
        NodeKind::ParenthesizedType => TypeRef::Parenthesized(inner(
            ParenthesizedType::cast(tree, node).unwrap().inner(tree),
        )),
        NodeKind::NullableType => {
            TypeRef::Nullable(inner(NullableType::cast(tree, node).unwrap().inner(tree)))
        }
        NodeKind::DefaultUnionType => TypeRef::Default(inner(
            DefaultUnionType::cast(tree, node).unwrap().inner(tree),
        )),
        NodeKind::UnionType => {
            let ty = UnionType::cast(tree, node).unwrap();
            TypeRef::Union(inner(ty.left(tree)), inner(ty.right(tree)))
        }
        NodeKind::FunctionType => {
            let ty = FunctionType::cast(tree, node).unwrap();
            TypeRef::Function {
                parameters: ty
                    .parameters(tree)
                    .map(|n| lower_type(db, tree, n))
                    .collect(),
                result: inner(ty.return_type(tree)),
            }
        }
        NodeKind::ConstrainedType => {
            let ty = ConstrainedType::cast(tree, node).unwrap();
            TypeRef::Constrained {
                base: inner(ty.inner(tree)),
                constraints: ty.constraints(tree).map(|n| opaque(tree, n)).collect(),
            }
        }
        _ => TypeRef::Unknown,
    }
}
// Constraints are opaque source text: punctuation and AST shape are semantic.
fn opaque(tree: &SyntaxTree, node: NodeId) -> String {
    tree.text(node).to_owned()
}
fn annotation(db: &dyn Db, tree: &SyntaxTree, node: NodeId) -> Option<TypeRef> {
    tree.child(node, |k| matches!(k, NodeKind::TypeAnnotation))
        .and_then(|n| TypeAnnotation::cast(tree, n)?.ty(tree))
        .map(|n| lower_type(db, tree, n))
}
fn type_parameters(db: &dyn Db, tree: &SyntaxTree, node: NodeId) -> Vec<TypeParameter> {
    tree.child(node, |k| matches!(k, NodeKind::TypeParameterList))
        .into_iter()
        .flat_map(|n| TypeParameterList::cast(tree, n).unwrap().parameters(tree))
        .filter_map(|n| {
            Some(TypeParameter {
                name: Name::new(db, tree.name(n)?),
                variance: lspkl_syntax::ast::nodes::TypeParameter::cast(tree, n)?.variance(tree),
            })
        })
        .collect()
}
fn lower(db: &dyn Db, file: File) -> ModuleItems {
    let mut items = ModuleItems::default();
    if file.status(db) == lspkl_db::FileStatus::Missing {
        return items;
    }
    let Ok(tree) = lspkl_db::parse(db, file) else {
        return items;
    };
    let Some(module) = Module::cast(tree, tree.root()) else {
        return items;
    };
    items.name = module.name(tree).map(|s| Name::new(db, s));
    items.extends = tree.module_extends_or_amends_target();
    items.amends = tree.is_amend();
    fn member(
        db: &dyn Db,
        tree: &SyntaxTree,
        node: NodeId,
        items: &mut ModuleItems,
    ) -> Option<ItemId> {
        let declaration = Declaration {
            name: Name::new(db, tree.name(node)?),
            modifiers: modifiers(tree, node),
        };
        let id = match tree.kind(node) {
            NodeKind::Class => {
                let class = Class::cast(tree, node).unwrap();
                let clause = tree.child(node, |k| matches!(k, NodeKind::ClassExtendsClause));
                let extends = clause.map(|n| {
                    let clause = ClassExtendsClause::cast(tree, n).unwrap();
                    TypeRef::Named {
                        path: clause
                            .name(tree)
                            .map(|n| path(db, tree, n))
                            .unwrap_or_default(),
                        arguments: clause
                            .arguments(tree)
                            .into_iter()
                            .flat_map(|n| TypeArgumentList::cast(tree, n).unwrap().arguments(tree))
                            .map(|n| lower_type(db, tree, n))
                            .collect(),
                    }
                });
                let index = items.classes.alloc(ClassDecl {
                    declaration,
                    type_parameters: type_parameters(db, tree, node),
                    extends,
                    members: Vec::new(),
                });
                let members = class
                    .body(tree)
                    .into_iter()
                    .flat_map(|n| ClassBody::cast(tree, n).unwrap().members(tree))
                    .filter_map(|n| member(db, tree, n, items))
                    .collect();
                items.classes[index].members = members;
                ItemId::Class(index)
            }
            NodeKind::TypeAlias => ItemId::TypeAlias(
                items.type_aliases.alloc(TypeAliasDecl {
                    declaration,
                    type_parameters: type_parameters(db, tree, node),
                    ty: TypeAlias::cast(tree, node)
                        .unwrap()
                        .ty(tree)
                        .map(|n| lower_type(db, tree, n))
                        .unwrap_or(TypeRef::Unknown),
                }),
            ),
            NodeKind::ClassProperty => ItemId::Property(items.properties.alloc(PropertyDecl {
                declaration,
                ty: annotation(db, tree, node),
            })),
            NodeKind::ClassMethod => {
                let header = tree.child(node, |k| matches!(k, NodeKind::MethodHeader))?;
                let method = MethodHeader::cast(tree, header).unwrap();
                let parameters = method
                    .parameters(tree)
                    .into_iter()
                    .flat_map(|n| ParameterList::cast(tree, n).unwrap().parameters(tree))
                    .map(|n| Parameter {
                        name: tree.name(n).map(|s| Name::new(db, s)),
                        ty: annotation(db, tree, n),
                    })
                    .collect();
                ItemId::Method(items.methods.alloc(MethodDecl {
                    declaration,
                    type_parameters: type_parameters(db, tree, header),
                    parameters,
                    result: annotation(db, tree, header),
                }))
            }
            _ => return None,
        };
        Some(id)
    }
    for node in module.members(tree) {
        if let Some(id) = member(db, tree, node, &mut items) {
            items.declarations.push(id);
        }
    }
    for node in module.imports(tree) {
        let uri_node = Import::cast(tree, node)
            .and_then(|n| n.uri(tree))
            .or_else(|| ImportGlob::cast(tree, node).and_then(|n| n.uri(tree)));
        let Some(uri) = uri_node.and_then(|n| tree.escaped_string_literal(n)) else {
            continue;
        };
        let name = tree
            .name(node)
            .unwrap_or_else(|| crate::resolver::module_uri_name(&uri));
        items.imports.alloc(ImportDecl {
            name: Name::new(db, name),
            uri,
            glob: tree.is_glob_import(node),
        });
    }
    items
}
#[salsa::tracked(returns(ref))]
fn declaration_data(db: &dyn Db, file: File) -> Arc<ModuleItems> {
    Arc::new(lower(db, file))
}
/// Structural equality is the firewall between text edits and semantic tables.
#[salsa::tracked(returns(ref))]
pub fn module_items(db: &dyn Db, file: File) -> Arc<ModuleItems> {
    declaration_data(db, file).clone()
}
#[salsa::tracked(returns(ref))]
pub fn item_source_map(db: &dyn Db, file: File) -> ItemSourceMap {
    let _ = file.text(db);
    let mut map = ItemSourceMap::default();
    if file.status(db) == lspkl_db::FileStatus::Missing {
        return map;
    }
    let Ok(tree) = lspkl_db::parse(db, file) else {
        return map;
    };
    let Some(module) = Module::cast(tree, tree.root()) else {
        return map;
    };
    let items = module_items(db, file);
    fn member(tree: &SyntaxTree, node: NodeId, counters: &mut [u32; 4], map: &mut ItemSourceMap) {
        if tree.name(node).is_none() {
            return;
        }
        let (slot, kind) = match tree.kind(node) {
            NodeKind::Class => (0, MemberKind::Class),
            NodeKind::TypeAlias => (1, MemberKind::TypeAlias),
            NodeKind::ClassProperty => (2, MemberKind::Property),
            NodeKind::ClassMethod
                if tree
                    .child(node, |k| matches!(k, NodeKind::MethodHeader))
                    .is_some() =>
            {
                (3, MemberKind::Method)
            }
            _ => return,
        };
        let raw = la_arena::RawIdx::from_u32(counters[slot]);
        counters[slot] += 1;
        let item = match kind {
            MemberKind::Class => ItemId::Class(Idx::from_raw(raw)),
            MemberKind::TypeAlias => ItemId::TypeAlias(Idx::from_raw(raw)),
            MemberKind::Property => ItemId::Property(Idx::from_raw(raw)),
            MemberKind::Method => ItemId::Method(Idx::from_raw(raw)),
        };
        map.nodes.insert(item, node);
        map.declarations.insert(node, item);
        if let Some(class) = Class::cast(tree, node) {
            for child in class
                .body(tree)
                .into_iter()
                .flat_map(|n| ClassBody::cast(tree, n).unwrap().members(tree))
            {
                member(tree, child, counters, map);
            }
        }
    }
    let mut counters = [0; 4];
    for node in module.members(tree) {
        member(tree, node, &mut counters, &mut map);
    }
    let mut imports = items.imports.iter();
    for node in module.imports(tree) {
        let uri = Import::cast(tree, node)
            .and_then(|n| n.uri(tree))
            .or_else(|| ImportGlob::cast(tree, node).and_then(|n| n.uri(tree)));
        if uri.and_then(|n| tree.escaped_string_literal(n)).is_some()
            && let Some((id, _)) = imports.next()
        {
            let item = ItemId::Import(id);
            map.nodes.insert(item, node);
            map.declarations.insert(node, item);
        }
    }
    map
}
pub fn item_node(db: &dyn Db, declaration: DeclId) -> Option<NodeRef> {
    item_source_map(db, declaration.file)
        .nodes
        .get(&declaration.item)
        .copied()
        .map(|n| NodeRef::new(declaration.file, n))
}
pub fn decl_for_node(db: &dyn Db, node: NodeRef, root: File) -> Option<DeclId> {
    item_source_map(db, node.file)
        .declarations
        .get(&node.value)
        .copied()
        .map(|item| DeclId {
            file: node.file,
            item,
            root: crate::member_queries::module_id(db, node.file, root).root(db),
        })
}
pub fn import_declarations(
    db: &dyn Db,
    file: File,
    root: File,
) -> impl Iterator<Item = (Name, DeclId)> + '_ {
    module_items(db, file)
        .imports
        .iter()
        .map(move |(id, import)| {
            (
                import.name,
                DeclId {
                    file,
                    item: ItemId::Import(id),
                    root: crate::member_queries::module_id(db, file, root).root(db),
                },
            )
        })
}
impl ModuleItems {
    pub fn declaration(&self, item: ItemId) -> Option<(&Declaration, MemberKind)> {
        Some(match item {
            ItemId::Class(i) => (
                &arena_item(&self.classes, i)?.declaration,
                MemberKind::Class,
            ),
            ItemId::TypeAlias(i) => (
                &arena_item(&self.type_aliases, i)?.declaration,
                MemberKind::TypeAlias,
            ),
            ItemId::Property(i) => (
                &arena_item(&self.properties, i)?.declaration,
                MemberKind::Property,
            ),
            ItemId::Method(i) => (
                &arena_item(&self.methods, i)?.declaration,
                MemberKind::Method,
            ),
            ItemId::Import(_) => return None,
        })
    }
}
pub(crate) fn arena_item<T>(arena: &Arena<T>, index: Idx<T>) -> Option<&T> {
    ((index.into_raw().into_u32() as usize) < arena.len()).then(|| &arena[index])
}
