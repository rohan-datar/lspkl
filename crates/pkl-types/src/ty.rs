//! The resolved type system.

use std::collections::{HashMap, HashSet};

use crate::constraint::ConstraintExpr;
use crate::context::AnalysisContext;
use crate::file_id::NodeRef;
use crate::resolver;
use crate::to_type::to_type;

/// A resolved type. Names are already resolved to declarations; aliases are
/// preserved for display but transparent for subtyping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    /// Top-ish: assignable to/from anything. Indicates inference failure or an
    /// unresolved name. Suppresses cascading errors.
    Unknown,

    /// Bottom: subtype of every type, no values. Result of `throw`, etc.
    Nothing,

    /// A type variable (generic parameter), e.g. `T` in `class Foo<T>`.
    Variable {
        type_param: NodeRef,
        constraints: Vec<ConstraintExpr>,
    },

    /// A concrete class with optional type arguments. Examples: `String`,
    /// `Listing<Int>`, `Mapping<String, Duration>`.
    Class {
        class_node: NodeRef,
        type_args: Vec<Type>,
        constraints: Vec<ConstraintExpr>,
    },

    /// A module used as a type.
    Module {
        module_node: NodeRef,
        reference_name: String,
        constraints: Vec<ConstraintExpr>,
    },

    /// A type alias. Transparent for subtyping (resolves to the aliased type),
    /// preserved for display so error messages match what the user wrote.
    Alias {
        alias_node: NodeRef,
        type_args: Vec<Type>,
        constraints: Vec<ConstraintExpr>,
    },

    /// `A | B`. Nullable types like `Foo?` desugar to `Union(Null, Foo)`.
    Union {
        left: Box<Type>,
        right: Box<Type>,
        constraints: Vec<ConstraintExpr>,
    },

    /// A string literal type, e.g. `"hello"` used as a type. Subtype of
    /// `String`.
    StringLiteral {
        value: String,
        constraints: Vec<ConstraintExpr>,
    },
}

impl Type {
    // ---- Constructors ----

    pub fn class(class_node: NodeRef) -> Self {
        Type::Class {
            class_node,
            type_args: Vec::new(),
            constraints: Vec::new(),
        }
    }

    pub fn class_with_args(class_node: NodeRef, type_args: Vec<Type>) -> Self {
        Type::Class {
            class_node,
            type_args,
            constraints: Vec::new(),
        }
    }

    pub fn string_literal(value: impl Into<String>) -> Self {
        Type::StringLiteral {
            value: value.into(),
            constraints: Vec::new(),
        }
    }

    /// Return `self` with contraints attached. No-op for `Unknown` and `Nothing`
    pub fn with_constraints(self, constraints: Vec<ConstraintExpr>) -> Type {
        match self {
            Type::Variable { type_param, .. } => Type::Variable {
                type_param,
                constraints,
            },
            Type::Class {
                class_node,
                type_args,
                ..
            } => Type::Class {
                class_node,
                type_args,
                constraints,
            },
            Type::Module {
                module_node,
                reference_name,
                ..
            } => Type::Module {
                module_node,
                reference_name,
                constraints,
            },
            Type::Alias {
                alias_node,
                type_args,
                ..
            } => Type::Alias {
                alias_node,
                type_args,
                constraints,
            },
            Type::Union { left, right, .. } => Type::Union {
                left,
                right,
                constraints,
            },
            Type::StringLiteral { value, .. } => Type::StringLiteral { value, constraints },
            _ => self,
        }
    }

    pub fn with_type_args(self, type_args: Vec<Type>) -> Type {
        match self {
            Type::Class {
                class_node,
                constraints,
                ..
            } => Type::Class {
                class_node,
                type_args,
                constraints,
            },
            Type::Alias {
                alias_node,
                constraints,
                ..
            } => Type::Alias {
                alias_node,
                type_args,
                constraints,
            },
            _ => self,
        }
    }

    /// True iff `self` and `other` are both [`Type::Class`] referring to the
    /// same class declaration. Compares `class_node` (a [`NodeRef`]) on both
    /// sides; type arguments and constraints are ignored.
    pub fn class_equals(&self, other: &Type) -> bool {
        match (self, other) {
            (Type::Class { class_node: a, .. }, Type::Class { class_node: b, .. }) => a == b,
            _ => false,
        }
    }

    /// Return `self` with contraints attached. No-op for `Unknown` and `Nothing`
    pub fn constraints(&self) -> Vec<ConstraintExpr> {
        match self {
            Type::Variable { constraints, .. }
            | Type::Class { constraints, .. }
            | Type::Module { constraints, .. }
            | Type::Alias { constraints, .. }
            | Type::Union { constraints, .. }
            | Type::StringLiteral { constraints, .. } => constraints.clone(),
            _ => vec![],
        }
    }

    /// Is `self` a (non-strict) subtype of `other`?
    pub fn is_subtype_of(&self, other: &Type, ctx: &AnalysisContext) -> bool {
        match (self, other) {
            (Type::Unknown, _) | (_, Type::Unknown) => true,
            (Type::Nothing, _) => true,
            (_, Type::Nothing) => false,
            (Type::Class { class_node: a, .. }, Type::Class { class_node: b, .. }) => {
                a == b || resolver::is_subclass(*a, *b, ctx)
            }
            // TODO: Handle generic types
            // (
            //     Type::Class {
            //         class_node: a,
            //         type_args: a_args,
            //         ..
            //     },
            //     Type::Class {
            //         class_node: b,
            //         type_args: b_args,
            //         ..
            //     },
            // ) => resolver::is_subclass(*a, *b) && type_args_match(a_args, b_args, ctx),
            (Type::StringLiteral { .. }, Type::Class { .. }) => {
                other.class_equals(ctx.base.string_type())
                    || other.class_equals(ctx.base.any_type())
            }
            (_, Type::Union { left, right, .. }) => {
                self.is_subtype_of(left, ctx) || self.is_subtype_of(right, ctx)
            }

            (Type::Union { left, right, .. }, _) => {
                left.is_subtype_of(other, ctx) && right.is_subtype_of(other, ctx)
            }
            (Type::Alias { .. }, _) => self.unaliased(ctx).is_subtype_of(other, ctx),
            (_, Type::Alias { .. }) => self.is_subtype_of(&other.unaliased(ctx), ctx),
            _ => false,
        }
    }

    /// Render this type to a human-readable string for hover/diagnostics.
    pub fn render(&self, ctx: &AnalysisContext) -> String {
        match self {
            Type::Unknown => String::from("unknown"),
            Type::Nothing => String::from("nothing"),
            Type::Class {
                class_node,
                type_args,
                constraints,
            } => render_resolved_type(ctx, class_node, type_args, constraints),
            Type::StringLiteral { value, .. } => format!("\"{value}\""),
            Type::Alias {
                alias_node,
                type_args,
                constraints,
            } => render_resolved_type(ctx, alias_node, type_args, constraints),
            Type::Union { left, right, .. } => {
                if left.class_equals(ctx.base.null_type()) {
                    format!("{}?", right.render(ctx))
                } else if right.class_equals(ctx.base.null_type()) {
                    format!("{}?", left.render(ctx))
                } else {
                    format!("{}|{}", left.render(ctx), right.render(ctx))
                }
            }
            Type::Module {
                reference_name,
                constraints,
                ..
            } => {
                let mut name = reference_name.clone();
                if !constraints.is_empty() {
                    name.push('(');
                    for constraint in constraints {
                        name.push_str(format!("{}", constraint).as_str());
                        name.push_str(", ");
                    }
                    name.push(')');
                }
                name
            }
            Type::Variable {
                type_param,
                constraints,
            } => {
                let mut name = String::from(
                    ctx.tree_for(type_param.file_id)
                        .expect("unable to find tree for file_id for rendering")
                        .name(type_param.node_id)
                        .unwrap_or(""),
                );
                if !constraints.is_empty() {
                    name.push('(');
                    for constraint in constraints {
                        name.push_str(format!("{}", constraint).as_str());
                        name.push_str(", ");
                    }
                    name.push(')');
                }
                name
            }
        }
    }

    /// True when `Null` is a subtype of `self` (i.e., the value can be null).
    pub fn is_nullable(&self, ctx: &AnalysisContext) -> bool {
        self.is_subtype_of(ctx.base.null_type(), ctx)
    }

    /// Strip `Null` out of a union. Returns `self` if not nullable.
    pub fn non_null(&self, ctx: &AnalysisContext) -> Type {
        match self {
            Type::Union { left, right, .. } => {
                if left.is_nullable(ctx) {
                    right.non_null(ctx)
                } else if right.is_nullable(ctx) {
                    left.non_null(ctx)
                } else {
                    self.clone()
                }
            }
            _ => self.clone(),
        }
    }

    /// Wrap `self` in `Union(Null, self)` to make it nullable.
    pub fn nullable(&self, ctx: &AnalysisContext) -> Type {
        Type::Union {
            left: Box::new(ctx.base.null_type().clone()),
            right: Box::new(self.clone()),
            constraints: vec![],
        }
    }

    /// If `self` is an alias, resolve through to the underlying type. Otherwise
    /// returns a clone.
    pub fn unaliased(&self, ctx: &AnalysisContext) -> Type {
        let mut current = self.clone();
        let mut seen: HashSet<NodeRef> = HashSet::new();
        while let Type::Alias {
            alias_node,
            type_args,
            constraints,
        } = current
        {
            // TODO add generics
            // TODO chain outer alias constriants
            let _ = (type_args, constraints);
            if !seen.insert(alias_node) {
                return Type::Unknown;
            }
            let Some(tree) = ctx.tree_for(alias_node.file_id) else {
                return Type::Unknown;
            };
            let Some(body) = tree.type_alias_body(alias_node.node_id) else {
                return Type::Unknown;
            };
            current = to_type(body, &TypeBindings::new(), ctx);
        }
        current
    }
}

/// Type-parameter substitution table. Maps a type-parameter declaration's
/// `NodeRef` to the concrete `Type` it has been bound to.
pub type TypeBindings = HashMap<NodeRef, Type>;

fn render_resolved_type(
    ctx: &AnalysisContext,
    node: &NodeRef,
    type_args: &Vec<Type>,
    constraints: &Vec<ConstraintExpr>,
) -> String {
    let mut resolved = String::from(
        ctx.tree_for(node.file_id)
            .expect("unable to find tree for file_id for rendering")
            .name(node.node_id)
            .unwrap_or(""),
    );
    if !type_args.is_empty() {
        resolved.push('<');
        for arg in type_args {
            resolved.push_str(&arg.render(ctx));
            resolved.push_str(", ");
        }
        resolved.push('>');
    }

    if !constraints.is_empty() {
        resolved.push('(');
        for constraint in constraints {
            resolved.push_str(format!("{}", constraint).as_str());
            resolved.push_str(", ");
        }
        resolved.push(')');
    }
    resolved
}
