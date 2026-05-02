//! pkl-analysis: parsing, AST, type system, and diagnostics for the Pkl language.

pub mod ast;
mod parser;

pub use parser::{parse, ParseError};
