//! Bundled context passed to type-system functions.

use std::collections::HashMap;
use std::sync::Arc;

use pkl_syntax::ast::SyntaxTree;

use crate::base_module::BaseModule;
use crate::file_id::{FileId, NodeRef};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportResolution {
    Single(NodeRef),
    Glob(Vec<NodeRef>),
    Unresolved { package_to_download: Option<String> },
}

impl ImportResolution {
    pub fn single(&self) -> Option<NodeRef> {
        match self {
            Self::Single(node) => Some(*node),
            Self::Glob(_) | Self::Unresolved { .. } => None,
        }
    }
}

/// Thin bundle of "everything a type-system function needs to look things up".
///
/// Construct fresh on each top-level entry point; cheap to pass by value
/// because the fields are all references (and `FileId` is `Copy`).
#[derive(Clone, Copy)]
pub struct AnalysisContext<'a> {
    /// The syntax tree for `file_id`. Cross-file declarations are looked up
    /// through `tree_for`, using base or the request-pinned tree map.
    pub tree: &'a SyntaxTree,

    /// Which file `tree` belongs to. Used to construct `NodeRef`s for nodes
    /// found in `tree`.
    pub file_id: FileId,

    /// The standard library, loaded once at startup.
    pub base: &'a BaseModule,

    /// Request-pinned user trees, including reachable imports not open in the editor.
    pub trees: &'a HashMap<FileId, Arc<SyntaxTree>>,

    /// URI owners mapped to resolved pinned roots, empty globs, or explicit failures.
    pub imports: &'a HashMap<NodeRef, ImportResolution>,
}

impl<'a> AnalysisContext<'a> {
    pub fn new(
        tree: &'a SyntaxTree,
        file_id: FileId,
        base: &'a BaseModule,
        trees: &'a HashMap<FileId, Arc<SyntaxTree>>,
        imports: &'a HashMap<NodeRef, ImportResolution>,
    ) -> Self {
        Self {
            tree,
            file_id,
            base,
            trees,
            imports,
        }
    }

    /// Look up the syntax tree for an arbitrary `FileId`. Crucial when you
    /// hold a `NodeRef` whose `file_id` may be `FileId::BASE` (i.e. it points
    /// into the standard library) or the current user file.
    ///
    /// Returns `None` for a file outside this request. Current-file and BASE
    /// snapshots take precedence over entries in the borrowed tree map.
    pub fn tree_for(&self, file_id: FileId) -> Option<&SyntaxTree> {
        if file_id == self.base.file_id() {
            Some(self.base.tree().as_ref())
        } else if file_id == self.file_id {
            Some(self.tree)
        } else if let Some(tree) = self.trees.get(&file_id) {
            Some(tree)
        } else {
            None
        }
    }
}
