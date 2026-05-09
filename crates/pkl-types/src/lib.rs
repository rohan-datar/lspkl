//! Pkl type system: resolved types, subtyping, type inference.

pub mod base_module;
pub mod compute_expr;
pub mod context;
pub mod file_id;
pub mod resolver;
pub mod to_type;
pub mod ty;

pub use base_module::{BaseModule, BaseModuleError};
pub use context::AnalysisContext;
pub use file_id::{FileId, NodeRef};
pub use ty::{CompareOp, ConstraintExpr, ConstraintValue, Type, TypeBindings};
