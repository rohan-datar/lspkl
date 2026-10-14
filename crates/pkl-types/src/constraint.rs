//! Constraints attached to refined types like `Int(this >= 0)`.

use std::fmt::Display;

/// A constraint attached to a constrained type, e.g. `Int(this >= 0)`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ConstraintExpr {
    /// Could not analyze the constraint statically — hold onto the source text
    /// so it can be rendered.
    Opaque(String),
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
        }
    }
}
