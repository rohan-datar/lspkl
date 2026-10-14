//! Database-backed syntax and imports shared by semantic computations.
use crate::base_module::BaseModule;
use crate::file_id::NodeRef;
use lspkl_db::{Db, File};
use lspkl_syntax::ast::{NodeId, SyntaxTree};

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

/// Every syntax input carries its database identity.
#[derive(Clone, Copy)]
pub struct AnalysisContext<'a> {
    pub base: &'a BaseModule,
    pub db: &'a dyn Db,
    pub root: File,
}
impl<'a> AnalysisContext<'a> {
    /// Construct a semantic query context without caller-owned SDK state.
    pub fn for_root(db: &'a dyn Db, root: File) -> Self {
        Self {
            base: crate::base_module(db).as_ref(),
            db,
            root,
        }
    }
    pub fn tree(&self, file: File) -> Option<&'a SyntaxTree> {
        if file.status(self.db) == lspkl_db::FileStatus::Missing {
            return None;
        }
        lspkl_db::parse(self.db, file)
            .as_ref()
            .ok()
            .map(|tree| tree.as_ref())
    }
    pub fn import(&self, node: NodeRef) -> Option<ImportResolution> {
        let tree = self.tree(node.file)?;
        if !tree.kind(node.value).is_module_uri_owner() {
            return None;
        }
        Some(
            match lspkl_db::import_graph(self.db, self.root)
                .edges
                .get(&(node.file, node.value))?
            {
                lspkl_db::ImportResolution::Single(file) => {
                    ImportResolution::Single(NodeRef::new(*file, NodeId::ROOT))
                }
                lspkl_db::ImportResolution::Glob(files) => ImportResolution::Glob(
                    files
                        .iter()
                        .map(|file| NodeRef::new(*file, NodeId::ROOT))
                        .collect(),
                ),
                lspkl_db::ImportResolution::Unresolved {
                    package_to_download,
                } => ImportResolution::Unresolved {
                    package_to_download: package_to_download.clone(),
                },
            },
        )
    }
}
