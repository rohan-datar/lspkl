//! Pure compute for the LSP features lspkl exposes, in protocol types.
//!
//! The lspkl server is responsible for:
//! - resolving document URIs to database file inputs
//! - scheduling, cancellation and all I/O
//! - converting results and errors into JSON-RPC responses
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
pub mod diagnostics;
pub mod formatting;
pub mod goto_definition;
pub mod hover;
pub mod module_uri_completion;
