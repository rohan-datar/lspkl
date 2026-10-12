//! Request-pinned files and import edges shared by semantic computations.

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

/// No implicit current file: every syntax input carries its own identity.
#[derive(Clone, Copy)]
pub struct AnalysisContext<'a> {
    /// Well-known declarations cached from the pinned base module.
    pub base: &'a BaseModule,
    /// All request-pinned trees, including the base module.
    pub trees: &'a HashMap<FileId, Arc<SyntaxTree>>,
    /// URI owners mapped to resolved pinned roots or explicit failures.
    pub imports: &'a HashMap<NodeRef, ImportResolution>,
}

impl<'a> AnalysisContext<'a> {
    pub fn new(
        base: &'a BaseModule,
        trees: &'a HashMap<FileId, Arc<SyntaxTree>>,
        imports: &'a HashMap<NodeRef, ImportResolution>,
    ) -> Self {
        Self {
            base,
            trees,
            imports,
        }
    }

    /// Missing files are unavailable analysis, not a fallback to another tree.
    pub fn tree(&self, file: FileId) -> Option<&'a SyntaxTree> {
        self.trees.get(&file).map(Arc::as_ref)
    }
}
