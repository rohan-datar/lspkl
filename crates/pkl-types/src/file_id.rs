//! File-qualified syntax values and cross-file references.

use pkl_syntax::ast::{NodeId, SyntaxTree};

use crate::AnalysisContext;

/// Stable parsed-file identity, allocated by the server registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub u32);

impl FileId {
    /// Identity assigned when the embedded base module is registered.
    pub const BASE: FileId = FileId(0);
}

/// A syntax value paired with the file whose tree gives it meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InFile<T> {
    pub file: FileId,
    pub value: T,
}

impl<T> InFile<T> {
    pub fn new(file: FileId, value: T) -> Self {
        Self { file, value }
    }

    pub fn with_value<U>(self, value: U) -> InFile<U> {
        InFile::new(self.file, value)
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> InFile<U> {
        InFile::new(self.file, f(self.value))
    }
}

/// A raw arena reference. Typed views use the same file qualification.
pub type NodeRef = InFile<NodeId>;

impl NodeRef {
    /// Cast in this reference's pinned file, never an implicit current tree.
    pub fn cast<T>(
        self,
        ctx: &AnalysisContext,
        cast: impl FnOnce(&SyntaxTree, NodeId) -> Option<T>,
    ) -> Option<InFile<T>> {
        cast(ctx.tree(self.file)?, self.value).map(|value| self.with_value(value))
    }
}
