//! Constraints attached to refined types like `Int(this >= 0)`.

use std::fmt::Display;

/// A constraint attached to a constrained type, e.g. `Int(this >= 0)`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ConstraintExpr {
    /// Could not analyze the constraint statically — hold onto the source text
    /// so it can be rendered.
    Opaque(String),
    /// A simple comparison like `this >= 0`.
    Comparison {
        op: CompareOp,
        value: ConstraintValue,
    },
}

impl ConstraintExpr {
    /// Preserve the constraint expression verbatim.
    pub fn opaque(text: impl Into<String>) -> Self {
        ConstraintExpr::Opaque(text.into())
    }
}

impl Display for ConstraintExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConstraintExpr::Opaque(val) => write!(f, "{}", val),
            ConstraintExpr::Comparison { op, value } => write!(f, "this {} {}", op, value),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ConstraintValue {
    Int(i64),
    String(String),
    Bool(bool),
}

impl Display for ConstraintValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConstraintValue::Int(value) => write!(f, "{}", value),
            ConstraintValue::String(value) => write!(f, "{}", value),
            ConstraintValue::Bool(value) => write!(f, "{}", value),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CompareOp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl Display for CompareOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompareOp::Lt => write!(f, "<"),
            CompareOp::Le => write!(f, "<="),
            CompareOp::Gt => write!(f, ">"),
            CompareOp::Ge => write!(f, ">="),
            CompareOp::Eq => write!(f, "="),
            CompareOp::Ne => write!(f, "!="),
        }
    }
}
