//! pkl-syntax: parsing, AST, type system, and diagnostics for the Pkl language.

pub mod ast;
mod parser;

pub use line_index::{LineCol, LineIndex, WideEncoding, WideLineCol};
pub use parser::{ParseError, parse, parse_with_line_index};
pub use text_size::{TextRange, TextSize};
