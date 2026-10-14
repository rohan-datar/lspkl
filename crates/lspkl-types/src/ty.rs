//! Resolved, structurally comparable type values.

use crate::constraint::ConstraintExpr;
use crate::{
    BaseModule, ClassId, DeclId, ItemId, ModuleId, TypeParameterId, base_module, module_items,
    resolver,
};
use lspkl_db::{Db, File};
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashSet},
    fmt::Write,
    ops::Deref,
};

/// Canonical alternatives, independent of source order and union grouping.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UnionMembers(Vec<Type>);
impl UnionMembers {
    pub(crate) fn into_iter(self) -> std::vec::IntoIter<Type> {
        self.0.into_iter()
    }
}
impl Deref for UnionMembers {
    type Target = [Type];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Type identity uses declarations, never source positions. Routing context is
/// carried by the declaration/module IDs, including imported declarations.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Type {
    Unknown,
    Nothing,
    Variable {
        type_param: TypeParameterId,
    },
    Class {
        class_id: ClassId,
        type_args: Vec<Type>,
    },
    Module {
        module_id: ModuleId,
    },
    Alias {
        alias_id: DeclId,
        type_args: Vec<Type>,
    },
    Union {
        members: UnionMembers,
    },
    StringLiteral {
        value: String,
    },
    Constrained {
        base: Box<Type>,
        constraints: Vec<ConstraintExpr>,
    },
}

impl Type {
    pub fn class(class_id: ClassId) -> Self {
        Self::class_with_args(class_id, Vec::new())
    }
    pub fn class_with_args(class_id: ClassId, type_args: Vec<Type>) -> Self {
        Self::Class {
            class_id,
            type_args,
        }
    }
    pub fn string_literal(value: impl Into<String>) -> Self {
        Self::StringLiteral {
            value: value.into(),
        }
    }
    pub fn constrained(base: Type, mut constraints: Vec<ConstraintExpr>) -> Self {
        if constraints.is_empty() || matches!(base, Self::Unknown | Self::Nothing) {
            return base;
        }
        match base {
            Self::Constrained {
                base,
                constraints: mut inner,
            } => {
                inner.append(&mut constraints);
                Self::Constrained {
                    base,
                    constraints: inner,
                }
            }
            base => Self::Constrained {
                base: Box::new(base),
                constraints,
            },
        }
    }
    /// Flatten unions without erasing constraints, generic arguments or literal completions.
    pub fn union(types: impl IntoIterator<Item = Type>, db: &dyn Db) -> Self {
        Self::union_inner(types, db, &mut HashSet::new())
    }
    pub(crate) fn union_inner(
        types: impl IntoIterator<Item = Type>,
        db: &dyn Db,
        aliases: &mut HashSet<DeclId>,
    ) -> Self {
        let mut alternatives = Vec::new();
        for ty in types {
            match ty {
                Self::Nothing => {}
                Self::Unknown => return Self::Unknown,
                Self::Union { members } => alternatives.extend(members.0),
                ty => alternatives.push(ty),
            }
        }
        alternatives.sort_unstable();
        alternatives.dedup();
        for i in 0..alternatives.len() {
            if matches!(alternatives[i], Self::Nothing) {
                continue;
            }
            for j in 0..alternatives.len() {
                if i == j || matches!(alternatives[j], Self::Nothing) {
                    continue;
                }
                if can_absorb(&alternatives[i], &alternatives[j], db, aliases)
                    && (i < j || !can_absorb(&alternatives[j], &alternatives[i], db, aliases))
                {
                    alternatives[i] = Self::Nothing;
                    break;
                }
            }
        }
        alternatives.retain(|ty| !matches!(ty, Self::Nothing));
        match alternatives.len() {
            0 => Self::Nothing,
            1 => alternatives.pop().unwrap(),
            _ => Self::Union {
                members: UnionMembers(alternatives),
            },
        }
    }
    pub fn unconstrained(&self) -> &Self {
        match self {
            Self::Constrained { base, .. } => base.unconstrained(),
            ty => ty,
        }
    }
    pub fn constraints(&self) -> &[ConstraintExpr] {
        match self {
            Self::Constrained { constraints, .. } => constraints,
            _ => &[],
        }
    }
    pub fn has_constraints(&self, db: &dyn Db) -> bool {
        self.has_constraints_inner(db, &mut HashSet::new())
    }
    fn has_constraints_inner(&self, db: &dyn Db, aliases: &mut HashSet<DeclId>) -> bool {
        match self {
            Self::Constrained { base, constraints } => {
                !constraints.is_empty() || base.has_constraints_inner(db, aliases)
            }
            Self::Union { members } => members
                .iter()
                .any(|ty| ty.has_constraints_inner(db, aliases)),
            Self::Class { type_args, .. } => type_args
                .iter()
                .any(|ty| ty.has_constraints_inner(db, aliases)),
            Self::Alias {
                alias_id,
                type_args,
            } => {
                if aliases.contains(alias_id) {
                    return false;
                }
                let arguments: Vec<_> = type_args
                    .iter()
                    .map(|ty| ty.unaliased_inner(db, aliases))
                    .collect();
                aliases.insert(*alias_id);
                let expanded = expand_alias(db, *alias_id, &arguments, aliases);
                let result = expanded.has_constraints_inner(db, aliases);
                aliases.remove(alias_id);
                result
            }
            _ => false,
        }
    }
    pub fn with_type_args(self, type_args: Vec<Type>) -> Self {
        match self {
            Self::Class { class_id, .. } => Self::Class {
                class_id,
                type_args,
            },
            Self::Alias { alias_id, .. } => Self::Alias {
                alias_id,
                type_args,
            },
            Self::Constrained { base, constraints } => {
                Self::constrained(base.with_type_args(type_args), constraints)
            }
            ty => ty,
        }
    }
    pub fn class_equals(&self, other: &Self) -> bool {
        matches!((self.unconstrained(), other.unconstrained()),
            (Self::Class { class_id: a, .. }, Self::Class { class_id: b, .. }) if a == b)
    }
    pub fn is_subtype_of(&self, other: &Self, db: &dyn Db) -> bool {
        match (self.unconstrained(), other.unconstrained()) {
            (Self::Unknown, _) | (_, Self::Unknown) => true,
            (Self::Nothing, _) => true,
            (_, Self::Nothing) => false,
            (Self::Alias { .. }, _) => self.unaliased(db).is_subtype_of(other, db),
            (_, Self::Alias { .. }) => self.is_subtype_of(&other.unaliased(db), db),
            (Self::Union { members }, _) => members.iter().all(|ty| ty.is_subtype_of(other, db)),
            (_, Self::Union { members }) => members.iter().any(|ty| self.is_subtype_of(ty, db)),
            (Self::Class { class_id: a, .. }, Self::Class { class_id: b, .. }) => {
                a == b || resolver::is_subclass(*a, *b, db)
            }
            (Self::StringLiteral { value: a }, Self::StringLiteral { value: b }) => a == b,
            (Self::StringLiteral { .. }, Self::Class { .. }) => {
                let base = base_module(db);
                other.class_equals(base.string_type()) || other.class_equals(base.any_type())
            }
            (Self::Module { module_id: a }, Self::Module { module_id: b }) => {
                let mut current = Some(*a);
                let mut seen = HashSet::new();
                while let Some(module) = current {
                    if module == *b {
                        return true;
                    }
                    if !seen.insert(module) {
                        break;
                    }
                    current = resolver::module_super(module, db);
                }
                false
            }
            (Self::Module { .. }, Self::Class { .. }) => {
                let base = base_module(db);
                other.class_equals(base.module_type())
                    || other.class_equals(base.object_type())
                    || other.class_equals(base.any_type())
            }
            _ => false,
        }
    }
    pub fn has_common_subtype_with(&self, other: &Self, db: &dyn Db) -> bool {
        match (self.unconstrained(), other.unconstrained()) {
            (Self::Unknown | Self::Nothing, _) | (_, Self::Unknown | Self::Nothing) => false,
            (Self::Alias { .. }, _) => self.unaliased(db).has_common_subtype_with(other, db),
            (_, Self::Alias { .. }) => self.has_common_subtype_with(&other.unaliased(db), db),
            (Self::Union { members }, _) => members
                .iter()
                .any(|ty| ty.has_common_subtype_with(other, db)),
            (_, Self::Union { members }) => members
                .iter()
                .any(|ty| self.has_common_subtype_with(ty, db)),
            (Self::StringLiteral { value: a }, Self::StringLiteral { value: b }) => a == b,
            (Self::Variable { .. }, _) | (_, Self::Variable { .. }) => true,
            (Self::Class { class_id: a, .. }, Self::Class { class_id: b, .. }) => {
                resolver::is_subclass(*a, *b, db) || resolver::is_subclass(*b, *a, db)
            }
            (Self::Module { .. }, Self::Class { .. } | Self::Module { .. })
            | (Self::Class { .. }, Self::Module { .. }) => true,
            _ => false,
        }
    }
    pub fn render(&self, db: &dyn Db) -> String {
        let mut rendered = String::new();
        self.render_into(db, &mut rendered);
        rendered
    }
    fn render_into(&self, db: &dyn Db, out: &mut String) {
        match self {
            Self::Unknown => out.push_str("unknown"),
            Self::Nothing => out.push_str("nothing"),
            Self::Class {
                class_id,
                type_args,
            } => render_declaration(db, class_declaration(db, *class_id), type_args, out),
            Self::Alias {
                alias_id,
                type_args,
            } => render_declaration(db, *alias_id, type_args, out),
            Self::Module { module_id } => {
                if let Some(name) = module_items(db, module_id.file(db)).name {
                    out.push_str(name.text(db));
                }
            }
            Self::Variable { type_param } => {
                let params = crate::type_resolution::parameters(db, type_param.declaration);
                if let Some(param) = params.get(type_param.index as usize) {
                    out.push_str(param.name.text(db));
                }
            }
            Self::StringLiteral { value } => {
                out.push('"');
                out.push_str(value);
                out.push('"');
            }
            Self::Union { members } => {
                let base = base_module(db);
                let is_null =
                    |ty: &Type| ty.class_equals(base.null_type()) && ty.constraints().is_empty();
                let has_null = members.iter().any(is_null);
                let non_null_count = members
                    .iter()
                    .filter(|ty| !has_null || !is_null(ty))
                    .count();
                if has_null && non_null_count > 1 {
                    out.push('(');
                }
                for (i, ty) in members
                    .iter()
                    .filter(|ty| !has_null || !is_null(ty))
                    .enumerate()
                {
                    if i > 0 {
                        out.push('|');
                    }
                    ty.render_into(db, out);
                }
                if has_null && non_null_count > 1 {
                    out.push(')');
                }
                if has_null {
                    out.push('?');
                }
            }
            Self::Constrained { base, constraints } => {
                let needs_parens = matches!(base.as_ref(), Self::Union { .. });
                if needs_parens {
                    out.push('(');
                }
                base.render_into(db, out);
                if needs_parens {
                    out.push(')');
                }
                out.push('(');
                for (i, constraint) in constraints.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write!(out, "{constraint}").expect("writing into String cannot fail");
                }
                out.push(')');
            }
        }
    }
    pub fn is_nullable(&self, db: &dyn Db) -> bool {
        base_module(db).null_type().is_subtype_of(self, db)
    }
    pub fn non_null(&self, db: &dyn Db) -> Self {
        match self {
            Self::Union { members } => Self::union(members.iter().map(|ty| ty.non_null(db)), db),
            Self::Constrained { base, constraints } => {
                Self::constrained(base.non_null(db), constraints.clone())
            }
            Self::Alias { .. } => {
                let resolved = self.unaliased(db);
                if resolved.is_nullable(db) {
                    resolved.non_null(db)
                } else {
                    self.clone()
                }
            }
            ty if ty.class_equals(base_module(db).null_type()) => Self::Nothing,
            ty => ty.clone(),
        }
    }
    pub fn nullable(&self, db: &dyn Db) -> Self {
        Self::union([base_module(db).null_type().clone(), self.clone()], db)
    }
    pub fn is_non_null(&self, base: &BaseModule) -> bool {
        matches!((self.unconstrained(), base.non_null_type().unconstrained()),
            (Self::Alias { alias_id: a, .. }, Self::Alias { alias_id: b, .. }) if a == b)
    }
    pub fn unaliased(&self, db: &dyn Db) -> Self {
        self.unaliased_inner(db, &mut HashSet::new())
    }
    fn unaliased_inner(&self, db: &dyn Db, seen: &mut HashSet<DeclId>) -> Self {
        match self {
            Self::Constrained { base, constraints } => {
                Self::constrained(base.unaliased_inner(db, seen), constraints.clone())
            }
            Self::Alias {
                alias_id,
                type_args,
            } => {
                if seen.contains(alias_id) {
                    return Self::Unknown;
                }
                let arguments: Vec<_> = type_args
                    .iter()
                    .map(|ty| ty.unaliased_inner(db, seen))
                    .collect();
                seen.insert(*alias_id);
                let expanded = expand_alias(db, *alias_id, &arguments, seen);
                seen.remove(alias_id);
                expanded
            }
            Self::Union { members } => {
                let expanded = members
                    .iter()
                    .map(|ty| ty.unaliased_inner(db, seen))
                    .collect::<Vec<_>>();
                Self::union_inner(expanded, db, seen)
            }
            Self::Class {
                class_id,
                type_args,
            } => Self::class_with_args(
                *class_id,
                type_args
                    .iter()
                    .map(|ty| ty.unaliased_inner(db, seen))
                    .collect(),
            ),
            ty => ty.clone(),
        }
    }
    pub fn context_root(&self, db: &dyn Db) -> File {
        match self.unconstrained() {
            Self::Class { class_id, .. } => class_id.module(db).root(db),
            Self::Module { module_id } => module_id.root(db),
            Self::Alias { alias_id, .. } => alias_id.root,
            Self::Variable { type_param } => type_param.declaration.root,
            Self::Union { members } => members
                .first()
                .map_or_else(|| base_module(db).file_id(), |ty| ty.context_root(db)),
            _ => base_module(db).file_id(),
        }
    }
}

/// Ordered substitutions can be borrowed by interned query keys without copying.
pub(crate) type TypeBindings = BTreeMap<TypeParameterId, Type>;

fn class_declaration(db: &dyn Db, class: ClassId) -> DeclId {
    let module = class.module(db);
    DeclId {
        file: module.file(db),
        root: module.root(db),
        item: ItemId::Class(class.index(db)),
    }
}
fn resolved_for_normalization<'a>(
    ty: &'a Type,
    db: &dyn Db,
    aliases: &mut HashSet<DeclId>,
) -> Cow<'a, Type> {
    if matches!(ty.unconstrained(), Type::Alias { .. }) {
        Cow::Owned(ty.unaliased_inner(db, aliases))
    } else {
        Cow::Borrowed(ty)
    }
}
fn can_absorb(sub: &Type, sup: &Type, db: &dyn Db, aliases: &mut HashSet<DeclId>) -> bool {
    let sub = resolved_for_normalization(sub, db, aliases);
    let sup = resolved_for_normalization(sup, db, aliases);
    if matches!(sub.unconstrained(), Type::Unknown) || matches!(sup.unconstrained(), Type::Unknown)
    {
        return false;
    }
    if !sub.constraints().is_empty() && !sup.constraints().is_empty()
        || sup.class_equals(base_module(db).string_type()) && sup.constraints().is_empty()
        || !sup
            .constraints()
            .iter()
            .all(|c| sub.constraints().contains(c))
    {
        return false;
    }
    match (sub.unconstrained(), sup.unconstrained()) {
        (Type::Union { members }, _) => {
            return members.iter().all(|ty| can_absorb(ty, &sup, db, aliases));
        }
        (_, Type::Union { members }) => {
            return members.iter().any(|ty| can_absorb(&sub, ty, db, aliases));
        }
        _ => {}
    }
    if let (Type::Class { type_args: a, .. }, Type::Class { type_args: b, .. }) =
        (sub.unconstrained(), sup.unconstrained())
        && !b.is_empty()
        && a != b
    {
        return false;
    }
    sub.is_subtype_of(&sup, db)
}
fn expand_alias(db: &dyn Db, alias: DeclId, args: &[Type], seen: &mut HashSet<DeclId>) -> Type {
    crate::type_resolution::resolve_alias(db, alias, args).unaliased_inner(db, seen)
}
fn render_declaration(db: &dyn Db, declaration: DeclId, args: &[Type], out: &mut String) {
    let items = module_items(db, declaration.file);
    let name = match declaration.item {
        ItemId::Import(index) => {
            crate::items::arena_item(&items.imports, index).map(|import| import.name)
        }
        item => items
            .declaration(item)
            .map(|(declaration, _)| declaration.name),
    };
    if let Some(name) = name {
        out.push_str(name.text(db));
    }
    if !args.is_empty() {
        out.push('<');
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            arg.render_into(db, out);
        }
        out.push('>');
    }
}
