//! Bundled context passed to type-system functions.

use pkl_syntax::ast::SyntaxTree;

use crate::base_module::BaseModule;
use crate::file_id::FileId;

/// Thin bundle of "everything a type-system function needs to look things up".
///
/// Construct fresh on each top-level entry point; cheap to pass by value
/// because the fields are all references (and `FileId` is `Copy`).
#[derive(Clone, Copy)]
pub struct AnalysisContext<'a> {
    /// The syntax tree for `file_id`. Cross-file lookups (e.g. resolving a
    /// `class_node` that lives in `pkl:base`) go through `base` instead.
    pub tree: &'a SyntaxTree,

    /// Which file `tree` belongs to. Used to construct `NodeRef`s for nodes
    /// found in `tree`.
    pub file_id: FileId,

    /// The standard library, loaded once at startup.
    pub base: &'a BaseModule,
}

impl<'a> AnalysisContext<'a> {
    pub fn new(tree: &'a SyntaxTree, file_id: FileId, base: &'a BaseModule) -> Self {
        Self {
            tree,
            file_id,
            base,
        }
    }

    /// Look up the syntax tree for an arbitrary `FileId`. Crucial when you
    /// hold a `NodeRef` whose `file_id` may be `FileId::BASE` (i.e. it points
    /// into the standard library) or the current user file.
    ///
    /// Returns `None` for any file the context doesn't know about. Phase 6
    /// will extend this to consult a `FileManager` for arbitrary opened files;
    /// for now we only know about base + the current user tree.
    pub fn tree_for(&self, file_id: FileId) -> Option<&SyntaxTree> {
        if file_id == self.base.file_id() {
            Some(self.base.tree().as_ref())
        } else if file_id == self.file_id {
            Some(self.tree)
        } else {
            None
        }
    }
}
