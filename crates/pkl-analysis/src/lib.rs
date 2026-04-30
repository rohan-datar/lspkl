//! pkl-analysis: parsing, AST, type system, and diagnostics for the Pkl language.
//!
//! Phase 1 scope: parsing only. Provides:
//!   - [`parse`] — turn Pkl source into a [`SyntaxTree`].
//!   - [`ast`] — `NodeId`, `NodeKind`, `Span`, `SyntaxTree` and tree navigation.
//!
//! Higher layers (type system, diagnostics, LSP features) will live alongside this
//! module as further phases are implemented.

pub mod ast;
mod parser;

pub use parser::{ParseError, parse};
