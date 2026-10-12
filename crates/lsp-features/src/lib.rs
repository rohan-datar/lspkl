//! Pure compute for the LSP features lspkl exposes.
//!
//! The lspkl binary's `LanguageServer` handlers are responsible for:
//! - extracting state (looking up the right `VirtualFile` from a URI)
//! - any async / IO orchestration
//! - wrapping returned values in `jsonrpc::Result`
use tower_lsp_server::lsp_types::{Position, Range};
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
