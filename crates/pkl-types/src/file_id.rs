//! Cross-file references.
//!
//! Pkl types frequently reference declarations in other files (`String` lives
//! in `pkl:base`, an imported class lives in another `.pkl` file). Since our
//! AST is index-based, a `NodeId` alone is meaningless without knowing which
//! `SyntaxTree` it indexes into. [`NodeRef`] pairs the two.

use pkl_analysis::ast::NodeId;

/// Opaque identifier for a parsed file. The mapping from `FileId` to a
/// concrete `SyntaxTree` (or URL) is maintained outside this crate
///
/// Convention: [`FileId::BASE`] is reserved for the `pkl:base` standard library
/// module loaded by [`crate::BaseModule`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub u32);

impl FileId {
    /// Reserved id for the `pkl:base` stdlib module.
    pub const BASE: FileId = FileId(0);
}

/// A reference to a specific AST node in a specific file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeRef {
    pub file_id: FileId,
    pub node_id: NodeId,
}

impl NodeRef {
    pub fn new(file_id: FileId, node_id: NodeId) -> Self {
        Self { file_id, node_id }
    }
}
