//! Pure compute for the LSP features lspkl exposes.
//!
//! The lspkl binary's `LanguageServer` handlers are responsible for:
//! - resolving document URIs to database file inputs
//! - any async / IO orchestration
//! - wrapping returned values in `jsonrpc::Result`
use lsp_types::{Position, Range};
const MAX_RANGE: Range = Range {
    start: Position {
        line: 0,
        character: 0,
    },
    end: Position {
        line: u32::MAX,
        character: u32::MAX,
    },
};

pub mod positions;

pub mod code_action;
pub mod completion;
pub mod formatting;
pub mod goto_definition;
pub mod hover;
