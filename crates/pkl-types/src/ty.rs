//! Resolved, structurally comparable type values.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::ops::Deref;

use crate::constraint::ConstraintExpr;
use crate::context::AnalysisContext;
use crate::file_id::NodeRef;
use crate::resolver::is_subclass;
use crate::to_type::to_type_with_bindings_inner;
use crate::{BaseModule, resolver};

/// Alternatives constructed only by [`Type::union`]. Structural ordering makes
/// equality and hashing independent of source order and union grouping.
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

/// Names resolve to declarations. Aliases remain visible in display, but are
/// transparent for subtyping; presentation data is not part of module identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Type {
    Unknown,
    Nothing,
    Variable {
        type_param: NodeRef,
    },
    Class {
        class_node: NodeRef,
        type_args: Vec<Type>,
    },
    Module {
        module_node: NodeRef,
    },
    Alias {
        alias_node: NodeRef,
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
    pub fn class(class_node: NodeRef) -> Self {
        Self::class_with_args(class_node, Vec::new())
    }

    pub fn class_with_args(class_node: NodeRef, type_args: Vec<Type>) -> Self {
        Self::Class {
            class_node,
            type_args,
        }
    }

    pub fn string_literal(value: impl Into<String>) -> Self {
        Self::StringLiteral {
            value: value.into(),
        }
    }

    /// A refinement is a wrapper, not duplicated storage on every type variant.
    /// Nested refinements retain both sets of constraints.
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

    /// Flatten ordinary unions, remove bottom and duplicate alternatives, and
    /// absorb strict subtypes without losing constraints or literal completions.
    /// Unknown absorbs conservatively: its upstream equivalence-to-everything
    /// tie cases must not select a concrete type based on operand order.
    pub fn union(types: impl IntoIterator<Item = Type>, ctx: &AnalysisContext) -> Self {
        Self::union_inner(types, ctx, &mut HashSet::new())
    }

    pub(crate) fn union_inner(
        types: impl IntoIterator<Item = Type>,
        ctx: &AnalysisContext,
        aliases: &mut HashSet<NodeRef>,
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
        // Mark absorbed entries as bottom, then compact once rather than moving
        // the remaining alternatives after each removal.
        for i in 0..alternatives.len() {
            if matches!(alternatives[i], Self::Nothing) {
                continue;
            }
            for j in 0..alternatives.len() {
                if i == j || matches!(alternatives[j], Self::Nothing) {
                    continue;
                }
                if can_absorb(&alternatives[i], &alternatives[j], ctx, aliases)
                    && (i < j || !can_absorb(&alternatives[j], &alternatives[i], ctx, aliases))
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

    /// Refinements relevant to cast/type-test conclusions, including aliases,
    /// union alternatives and generic arguments. Alias traversal is cycle-safe.
    pub fn has_constraints(&self, ctx: &AnalysisContext) -> bool {
        self.has_constraints_inner(ctx, &mut HashSet::new())
    }

    fn has_constraints_inner(&self, ctx: &AnalysisContext, aliases: &mut HashSet<NodeRef>) -> bool {
        match self {
            Self::Constrained { base, constraints } => {
                !constraints.is_empty() || base.has_constraints_inner(ctx, aliases)
            }
            Self::Union { members } => members
                .iter()
                .any(|ty| ty.has_constraints_inner(ctx, aliases)),
            Self::Class { type_args, .. } => type_args
                .iter()
                .any(|ty| ty.has_constraints_inner(ctx, aliases)),
            Self::Alias {
                alias_node,
                type_args,
            } => {
                if aliases.contains(alias_node) {
                    return false;
                }
                // Caller arguments are finite values, not references introduced
                // by this alias body. Resolve them before entering its path.
                let arguments: Vec<_> = type_args
                    .iter()
                    .map(|ty| ty.unaliased_inner(ctx, aliases))
                    .collect();
                aliases.insert(*alias_node);
                let expanded = expand_alias(*alias_node, &arguments, ctx, aliases);
                let result = expanded.has_constraints_inner(ctx, aliases);
                aliases.remove(alias_node);
                result
            }
            _ => false,
        }
    }

    pub fn with_type_args(self, type_args: Vec<Type>) -> Self {
        match self {
            Self::Class { class_node, .. } => Self::Class {
                class_node,
                type_args,
            },
            Self::Alias { alias_node, .. } => Self::Alias {
                alias_node,
                type_args,
            },
            Self::Constrained { base, constraints } => {
                Self::constrained(base.with_type_args(type_args), constraints)
            }
            ty => ty,
        }
    }

    /// Compare class declarations, ignoring arguments and refinements.
    pub fn class_equals(&self, other: &Self) -> bool {
        match (self.unconstrained(), other.unconstrained()) {
            (Self::Class { class_node: a, .. }, Self::Class { class_node: b, .. }) => a == b,
            _ => false,
        }
    }

    /// Non-strict subtyping ignores refinements, as before this representation
    /// cutover. Normalization separately checks that absorption retains them.
    pub fn is_subtype_of(&self, other: &Self, ctx: &AnalysisContext) -> bool {
        match (self.unconstrained(), other.unconstrained()) {
            (Self::Unknown, _) | (_, Self::Unknown) => true,
            (Self::Nothing, _) => true,
            (_, Self::Nothing) => false,
            (Self::Alias { .. }, _) => self.unaliased(ctx).is_subtype_of(other, ctx),
            (_, Self::Alias { .. }) => self.is_subtype_of(&other.unaliased(ctx), ctx),
            (Self::Union { members }, _) => members.iter().all(|ty| ty.is_subtype_of(other, ctx)),
            (_, Self::Union { members }) => members.iter().any(|ty| self.is_subtype_of(ty, ctx)),
            (Self::Class { class_node: a, .. }, Self::Class { class_node: b, .. }) => {
                a == b || resolver::is_subclass(*a, *b, ctx)
            }
            (Self::StringLiteral { value: a }, Self::StringLiteral { value: b }) => a == b,
            (Self::StringLiteral { .. }, Self::Class { .. }) => {
                other.class_equals(ctx.base.string_type())
                    || other.class_equals(ctx.base.any_type())
            }
            (Self::Module { module_node: a }, Self::Module { module_node: b }) => {
                let mut current = Some(*a);
                let mut seen = HashSet::new();
                while let Some(node) = current {
                    if node == *b {
                        return true;
                    }
                    if !seen.insert(node) {
                        break;
                    }
                    current = resolver::module_super(node, ctx);
                }
                false
            }
            (Self::Module { .. }, Self::Class { .. }) => {
                other.class_equals(ctx.base.module_type())
                    || other.class_equals(ctx.base.object_type())
                    || other.class_equals(ctx.base.any_type())
            }
            _ => false,
        }
    }

    /// Whether a non-trivial inhabitant can belong to both types. Unknown and
    /// Nothing are not evidence of an intersection.
    pub fn has_common_subtype_with(&self, other: &Self, ctx: &AnalysisContext) -> bool {
        match (self.unconstrained(), other.unconstrained()) {
            (Self::Unknown | Self::Nothing, _) | (_, Self::Unknown | Self::Nothing) => false,
            (Self::Alias { .. }, _) => self.unaliased(ctx).has_common_subtype_with(other, ctx),
            (_, Self::Alias { .. }) => self.has_common_subtype_with(&other.unaliased(ctx), ctx),
            (Self::Union { members }, _) => members
                .iter()
                .any(|ty| ty.has_common_subtype_with(other, ctx)),
            (_, Self::Union { members }) => members
                .iter()
                .any(|ty| self.has_common_subtype_with(ty, ctx)),
            (Self::StringLiteral { value: a }, Self::StringLiteral { value: b }) => a == b,
            (Self::Variable { .. }, _) | (_, Self::Variable { .. }) => true,
            (Self::Class { class_node: a, .. }, Self::Class { class_node: b, .. }) => {
                is_subclass(*a, *b, ctx) || is_subclass(*b, *a, ctx)
            }
            (Self::Module { .. }, Self::Class { .. } | Self::Module { .. })
            | (Self::Class { .. }, Self::Module { .. }) => true,
            _ => false,
        }
    }

    pub fn render(&self, ctx: &AnalysisContext) -> String {
        let mut rendered = String::new();
        self.render_into(ctx, &mut rendered);
        rendered
    }

    fn render_into(&self, ctx: &AnalysisContext, out: &mut String) {
        match self {
            Self::Unknown => out.push_str("unknown"),
            Self::Nothing => out.push_str("nothing"),
            Self::Class {
                class_node,
                type_args,
            } => render_declaration(ctx, *class_node, type_args, out),
            Self::Alias {
                alias_node,
                type_args,
            } => render_declaration(ctx, *alias_node, type_args, out),
            Self::Module { module_node } => render_declaration(ctx, *module_node, &[], out),
            Self::Variable { type_param } => render_declaration(ctx, *type_param, &[], out),
            Self::StringLiteral { value } => {
                out.push('"');
                out.push_str(value);
                out.push('"');
            }
            Self::Union { members } => {
                let has_null = members
                    .iter()
                    .any(|ty| ty.class_equals(ctx.base.null_type()) && ty.constraints().is_empty());
                let non_null_count = members
                    .iter()
                    .filter(|ty| {
                        !has_null
                            || !ty.class_equals(ctx.base.null_type())
                            || !ty.constraints().is_empty()
                    })
                    .count();
                if has_null && non_null_count > 1 {
                    out.push('(');
                }
                let mut first = true;
                for ty in members.iter().filter(|ty| {
                    !has_null
                        || !ty.class_equals(ctx.base.null_type())
                        || !ty.constraints().is_empty()
                }) {
                    if !first {
                        out.push('|');
                    }
                    ty.render_into(ctx, out);
                    first = false;
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
                base.render_into(ctx, out);
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

    pub fn is_nullable(&self, ctx: &AnalysisContext) -> bool {
        ctx.base.null_type().is_subtype_of(self, ctx)
    }

    pub fn non_null(&self, ctx: &AnalysisContext) -> Self {
        match self {
            Self::Union { members } => Self::union(members.iter().map(|ty| ty.non_null(ctx)), ctx),
            Self::Constrained { base, constraints } => {
                Self::constrained(base.non_null(ctx), constraints.clone())
            }
            Self::Alias { .. } => {
                let resolved = self.unaliased(ctx);
                if resolved.is_nullable(ctx) {
                    resolved.non_null(ctx)
                } else {
                    self.clone()
                }
            }
            ty if ty.class_equals(ctx.base.null_type()) => Self::Nothing,
            ty => ty.clone(),
        }
    }

    pub fn nullable(&self, ctx: &AnalysisContext) -> Self {
        Self::union([ctx.base.null_type().clone(), self.clone()], ctx)
    }

    pub fn is_non_null(&self, base: &BaseModule) -> bool {
        match (self.unconstrained(), base.non_null_type().unconstrained()) {
            (Self::Alias { alias_node: a, .. }, Self::Alias { alias_node: b, .. }) => a == b,
            _ => false,
        }
    }

    /// Resolve alias chains while retaining refinements around the result.
    pub fn unaliased(&self, ctx: &AnalysisContext) -> Self {
        self.unaliased_inner(ctx, &mut HashSet::new())
    }

    fn unaliased_inner(&self, ctx: &AnalysisContext, seen: &mut HashSet<NodeRef>) -> Self {
        match self {
            Self::Constrained { base, constraints } => {
                Self::constrained(base.unaliased_inner(ctx, seen), constraints.clone())
            }
            Self::Alias {
                alias_node,
                type_args,
            } => {
                if seen.contains(alias_node) {
                    return Self::Unknown;
                }
                let arguments: Vec<_> = type_args
                    .iter()
                    .map(|ty| ty.unaliased_inner(ctx, seen))
                    .collect();
                seen.insert(*alias_node);
                let expanded = expand_alias(*alias_node, &arguments, ctx, seen);
                seen.remove(alias_node);
                expanded
            }
            Self::Union { members } => {
                let expanded: Vec<_> = members
                    .iter()
                    .map(|ty| ty.unaliased_inner(ctx, seen))
                    .collect();
                Self::union_inner(expanded, ctx, seen)
            }
            Self::Class {
                class_node,
                type_args,
            } => Self::class_with_args(
                *class_node,
                type_args
                    .iter()
                    .map(|ty| ty.unaliased_inner(ctx, seen))
                    .collect(),
            ),
            ty => ty.clone(),
        }
    }
}

/// Generic substitutions stay inside semantic recursion.
pub(crate) type TypeBindings = HashMap<NodeRef, Type>;

fn resolved_for_normalization<'a>(
    ty: &'a Type,
    ctx: &AnalysisContext,
    aliases: &mut HashSet<NodeRef>,
) -> Cow<'a, Type> {
    if matches!(ty.unconstrained(), Type::Alias { .. }) {
        Cow::Owned(ty.unaliased_inner(ctx, aliases))
    } else {
        Cow::Borrowed(ty)
    }
}

fn can_absorb(
    sub: &Type,
    sup: &Type,
    ctx: &AnalysisContext,
    aliases: &mut HashSet<NodeRef>,
) -> bool {
    let sub = resolved_for_normalization(sub, ctx, aliases);
    let sup = resolved_for_normalization(sup, ctx, aliases);
    if matches!(sub.unconstrained(), Type::Unknown) || matches!(sup.unconstrained(), Type::Unknown)
    {
        return false;
    }
    // Mirror Types.kt: two refined alternatives remain distinct, and a plain
    // String never absorbs literal alternatives needed by completion.
    if !sub.constraints().is_empty() && !sup.constraints().is_empty()
        || sup.class_equals(ctx.base.string_type()) && sup.constraints().is_empty()
        || !sup
            .constraints()
            .iter()
            .all(|c| sub.constraints().contains(c))
    {
        return false;
    }
    // Ordinary subtyping currently ignores generics. Do not use that leniency
    // to erase distinct arguments during normalization.
    match (sub.unconstrained(), sup.unconstrained()) {
        (Type::Union { members }, _) => {
            return members.iter().all(|ty| can_absorb(ty, &sup, ctx, aliases));
        }
        (_, Type::Union { members }) => {
            return members.iter().any(|ty| can_absorb(&sub, ty, ctx, aliases));
        }
        _ => {}
    }
    // The generic guard applies to each leaf, not only the outer alternatives.
    if let (Type::Class { type_args: a, .. }, Type::Class { type_args: b, .. }) =
        (sub.unconstrained(), sup.unconstrained())
        && !b.is_empty()
        && a != b
    {
        return false;
    }
    sub.is_subtype_of(&sup, ctx)
}

fn expand_alias(
    alias_node: NodeRef,
    type_args: &[Type],
    ctx: &AnalysisContext,
    aliases: &mut HashSet<NodeRef>,
) -> Type {
    let Some(tree) = ctx.tree(alias_node.file) else {
        return Type::Unknown;
    };
    let Some(alias) = pkl_syntax::ast::nodes::TypeAlias::cast(tree, alias_node.value) else {
        return Type::Unknown;
    };
    let Some(body) = alias.ty(tree) else {
        return Type::Unknown;
    };
    let mut bindings = TypeBindings::new();
    if let Some(parameters) = alias
        .type_parameters(tree)
        .and_then(|id| pkl_syntax::ast::nodes::TypeParameterList::cast(tree, id))
    {
        for (parameter, argument) in parameters.parameters(tree).zip(type_args) {
            bindings.insert(alias_node.with_value(parameter), argument.clone());
        }
    }
    to_type_with_bindings_inner(alias_node.with_value(body), &bindings, ctx, aliases)
        .unaliased_inner(ctx, aliases)
}

fn render_declaration(ctx: &AnalysisContext, node: NodeRef, args: &[Type], out: &mut String) {
    out.push_str(
        ctx.tree(node.file)
            .and_then(|tree| {
                if let Some(module) = pkl_syntax::ast::nodes::Module::cast(tree, node.value) {
                    module.name(tree)
                } else {
                    tree.name(node.value)
                }
            })
            .unwrap_or(""),
    );
    if !args.is_empty() {
        out.push('<');
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            arg.render_into(ctx, out);
        }
        out.push('>');
    }
}
