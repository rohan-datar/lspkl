//! The `pkl:base` standard library, loaded once at startup.

use std::sync::Arc;

use pkl_analysis::ast::{NodeId, NodeKind, SyntaxTree};
use thiserror::Error;

use crate::file_id::{FileId, NodeRef};
use crate::ty::Type;

#[derive(Debug, Error)]
pub enum BaseModuleError {
    #[error("failed to parse base.pkl: {0}")]
    Parse(#[from] pkl_analysis::ParseError),

    #[error("base.pkl is missing the required class `{0}`")]
    MissingClass(String),

    #[error("could not find pkl base module!")]
    NotFound(#[from] std::io::Error),
}

/// Resolved view of `pkl:base`.
#[derive(Debug)]
pub struct BaseModule {
    tree: Arc<SyntaxTree>,
    file_id: FileId,

    // Cached `Type` values for the classes the type checker references most
    // often. Populated by `load` so callers can grab them without re-walking
    // the AST. Add entries as analyzers need them.
    any_type: Type,
    null_type: Type,
    boolean_type: Type,
    int_type: Type,
    float_type: Type,
    number_type: Type,
    string_type: Type,
    listing_type: Type,
    list_type: Type,
    set_type: Type,
    mapping_type: Type,
    map_type: Type,
    object_type: Type,
    module_type: Type,
    bytes_type: Type,
    function_type: Type,
}

impl BaseModule {
    /// Parse `base.pkl` from a source string and resolve well-known types.
    pub fn load(source: &str) -> Result<Self, BaseModuleError> {
        let tree = Arc::new(pkl_analysis::parse(source)?);
        let classes = tree.children_matching(tree.root(), |kind| *kind == NodeKind::Class);

        Ok(Self {
            tree: tree.clone(),
            file_id: FileId::BASE,

            // well-known types
            any_type: find_class(tree.clone(), &classes, "Any")?,
            null_type: find_class(tree.clone(), &classes, "Null")?,
            boolean_type: find_class(tree.clone(), &classes, "Boolean")?,
            int_type: find_class(tree.clone(), &classes, "Int")?,
            float_type: find_class(tree.clone(), &classes, "Float")?,
            number_type: find_class(tree.clone(), &classes, "Number")?,
            string_type: find_class(tree.clone(), &classes, "String")?,
            bytes_type: find_class(tree.clone(), &classes, "Bytes")?,
            listing_type: find_class(tree.clone(), &classes, "Listing")?,
            list_type: find_class(tree.clone(), &classes, "List")?,
            set_type: find_class(tree.clone(), &classes, "Set")?,
            mapping_type: find_class(tree.clone(), &classes, "Mapping")?,
            map_type: find_class(tree.clone(), &classes, "Map")?,
            object_type: find_class(tree.clone(), &classes, "Object")?,
            module_type: find_class(tree.clone(), &classes, "Module")?,
            function_type: find_class(tree.clone(), &classes, "Function")?,
        })
    }

    /// Convenience: load from a path. Reads the file and delegates to [`Self::load`].
    pub fn load_from_path(path: &std::path::Path) -> Result<Self, BaseModuleError> {
        let text = std::fs::read_to_string(path)?;
        Self::load(&text)
    }

    pub fn tree(&self) -> &Arc<SyntaxTree> {
        &self.tree
    }

    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    /// Build a [`NodeRef`] that points into the base module.
    pub fn node_ref(&self, node_id: NodeId) -> NodeRef {
        NodeRef::new(self.file_id, node_id)
    }

    // ---- Well-known type accessors ----
    pub fn function_n_type(&self, n: usize) -> Option<Type> {
        if n > 5 {
            return None;
        }
        let fn_class_name = format!("Function{}", n);
        let tree = self.tree.clone();
        let classes = tree.children_matching(tree.root(), |kind| *kind == NodeKind::Class);
        let Ok(fn_class) = find_class(tree, &classes, fn_class_name.as_str()) else {
            return None;
        };
        Some(fn_class)
    }

    pub fn function_type(&self) -> &Type {
        &self.function_type
    }

    pub fn any_type(&self) -> &Type {
        &self.any_type
    }
    pub fn null_type(&self) -> &Type {
        &self.null_type
    }
    pub fn boolean_type(&self) -> &Type {
        &self.boolean_type
    }
    pub fn int_type(&self) -> &Type {
        &self.int_type
    }
    pub fn float_type(&self) -> &Type {
        &self.float_type
    }
    pub fn number_type(&self) -> &Type {
        &self.number_type
    }
    pub fn string_type(&self) -> &Type {
        &self.string_type
    }
    pub fn listing_type(&self) -> &Type {
        &self.listing_type
    }
    pub fn list_type(&self) -> &Type {
        &self.list_type
    }
    pub fn set_type(&self) -> &Type {
        &self.set_type
    }
    pub fn mapping_type(&self) -> &Type {
        &self.mapping_type
    }
    pub fn map_type(&self) -> &Type {
        &self.map_type
    }
    pub fn object_type(&self) -> &Type {
        &self.object_type
    }
    pub fn module_type(&self) -> &Type {
        &self.module_type
    }
    pub fn bytes_type(&self) -> &Type {
        &self.bytes_type
    }
}

fn find_class(
    tree: Arc<SyntaxTree>,
    classes: &[NodeId],
    name: &str,
) -> Result<Type, BaseModuleError> {
    let Some(id) = classes.iter().find(|c| tree.name(**c) == Some(name)) else {
        return Err(BaseModuleError::MissingClass(name.to_string()));
    };
    Ok(Type::class(NodeRef {
        file_id: FileId::BASE,
        node_id: *id,
    }))
}
