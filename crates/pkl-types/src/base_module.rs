//! The `pkl:base` standard library, loaded once at startup.

use std::sync::Arc;

use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree, unquote_identifier};
use thiserror::Error;

use crate::file_id::{FileId, NodeRef};
use crate::ty::Type;

#[derive(Debug, Error)]
pub enum BaseModuleError {
    #[error("failed to parse base.pkl: {0}")]
    Parse(#[from] pkl_syntax::ParseError),

    #[error("base.pkl is missing the required class `{0}`")]
    MissingClass(String),

    #[error("base.pkl is missing the required typealias `{0}`")]
    MissingAlias(String),

    #[error("base.pkl class `{class}` is missing the required member `{member}`")]
    MissingMember { class: String, member: String },
}

/// Resolved view of `pkl:base`.
#[derive(Debug)]
pub struct BaseModule {
    tree: Arc<SyntaxTree>,
    file_id: FileId,

    // Cached `Type` values for the classes the type checker references most
    // often. Populated by `load` so callers can grab them without re-walking
    // the AST.

    // classes
    any_type: Type,
    annotation_type: Type,
    dynamic_type: Type,
    null_type: Type,
    boolean_type: Type,
    int_type: Type,
    float_type: Type,
    duration_type: Type,
    data_size_type: Type,
    number_type: Type,
    string_type: Type,
    collection_type: Type,
    listing_type: Type,
    list_type: Type,
    set_type: Type,
    mapping_type: Type,
    map_type: Type,
    object_type: Type,
    module_type: Type,
    bytes_type: Type,
    function_type: Type,
    varargs_type: Type,

    // typealiases
    non_null_type: Type,
    comparable_type: Type,

    // methods
    listing_to_list_method: NodeRef,
    mapping_to_map_method: NodeRef,

    list_join_method: NodeRef,
    list_is_distinct_by_method: NodeRef,
    list_fold_method: NodeRef,
    list_fold_indexed_method: NodeRef,
    list_is_empty_property: NodeRef,
    list_length_property: NodeRef,
    list_is_distinct_property: NodeRef,

    map_contains_key_method: NodeRef,
    map_get_or_null_method: NodeRef,
    map_fold_method: NodeRef,
    map_is_empty_property: NodeRef,
    map_length_property: NodeRef,
    map_keys_property: NodeRef,
}

impl BaseModule {
    /// Parse `base.pkl` from a source string and resolve well-known types.
    pub fn load(source: &str) -> Result<Self, BaseModuleError> {
        let tree = Arc::new(pkl_syntax::parse(source)?);
        let classes = tree.children_matching(tree.root(), |kind| *kind == NodeKind::Class);

        let aliases = tree.children_matching(tree.root(), |kind| *kind == NodeKind::TypeAlias);

        Ok(Self {
            tree: tree.clone(),
            file_id: FileId::BASE,

            // well-known types
            any_type: find_class(tree.clone(), &classes, "Any")?,
            annotation_type: find_class(tree.clone(), &classes, "Annotation")?,
            dynamic_type: find_class(tree.clone(), &classes, "Dynamic")?,
            null_type: find_class(tree.clone(), &classes, "Null")?,
            boolean_type: find_class(tree.clone(), &classes, "Boolean")?,
            int_type: find_class(tree.clone(), &classes, "Int")?,
            float_type: find_class(tree.clone(), &classes, "Float")?,
            duration_type: find_class(tree.clone(), &classes, "Duration")?,
            data_size_type: find_class(tree.clone(), &classes, "DataSize")?,
            number_type: find_class(tree.clone(), &classes, "Number")?,
            string_type: find_class(tree.clone(), &classes, "String")?,
            bytes_type: find_class(tree.clone(), &classes, "Bytes")?,
            collection_type: find_class(tree.clone(), &classes, "Collection")?,
            listing_type: find_class(tree.clone(), &classes, "Listing")?,
            list_type: find_class(tree.clone(), &classes, "List")?,
            set_type: find_class(tree.clone(), &classes, "Set")?,
            mapping_type: find_class(tree.clone(), &classes, "Mapping")?,
            map_type: find_class(tree.clone(), &classes, "Map")?,
            object_type: find_class(tree.clone(), &classes, "Object")?,
            module_type: find_class(tree.clone(), &classes, "Module")?,
            function_type: find_class(tree.clone(), &classes, "Function")?,
            non_null_type: find_alias(tree.clone(), &aliases, "NonNull")?,
            comparable_type: find_alias(tree.clone(), &aliases, "Comparable")?,
            varargs_type: find_class(tree.clone(), &classes, "VarArgs")?,

            // conversion methods
            listing_to_list_method: find_class_member(tree.clone(), &classes, "Listing", "toList")?,
            mapping_to_map_method: find_class_member(tree.clone(), &classes, "Mapping", "toMap")?,

            // List members duplicated on Listing
            list_join_method: find_class_member(tree.clone(), &classes, "List", "join")?,
            list_is_distinct_by_method: find_class_member(
                tree.clone(),
                &classes,
                "List",
                "isDistinctBy",
            )?,
            list_fold_method: find_class_member(tree.clone(), &classes, "List", "fold")?,
            list_fold_indexed_method: find_class_member(
                tree.clone(),
                &classes,
                "List",
                "foldIndexed",
            )?,
            list_is_empty_property: find_class_member(tree.clone(), &classes, "List", "isEmpty")?,
            list_length_property: find_class_member(tree.clone(), &classes, "List", "length")?,
            list_is_distinct_property: find_class_member(
                tree.clone(),
                &classes,
                "List",
                "isDistinct",
            )?,

            // Map members duplicated on Mapping
            map_contains_key_method: find_class_member(
                tree.clone(),
                &classes,
                "Map",
                "containsKey",
            )?,
            map_get_or_null_method: find_class_member(tree.clone(), &classes, "Map", "getOrNull")?,
            map_fold_method: find_class_member(tree.clone(), &classes, "Map", "fold")?,
            map_is_empty_property: find_class_member(tree.clone(), &classes, "Map", "isEmpty")?,
            map_length_property: find_class_member(tree.clone(), &classes, "Map", "length")?,
            map_keys_property: find_class_member(tree.clone(), &classes, "Map", "keys")?,
        })
    }

    /// Load `pkl:base` from the embedded standard-library artifact.
    pub fn load_embedded() -> Result<Self, BaseModuleError> {
        Self::load(pkl_stdlib::BASE)
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
    pub fn annotation_type(&self) -> &Type {
        &self.annotation_type
    }
    pub fn dynamic_type(&self) -> &Type {
        &self.dynamic_type
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
    pub fn duration_type(&self) -> &Type {
        &self.duration_type
    }
    pub fn data_size_type(&self) -> &Type {
        &self.data_size_type
    }
    pub fn number_type(&self) -> &Type {
        &self.number_type
    }
    pub fn string_type(&self) -> &Type {
        &self.string_type
    }
    pub fn collection_type(&self) -> &Type {
        &self.collection_type
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
    pub fn varargs_type(&self) -> &Type {
        &self.varargs_type
    }

    // Type Aliases
    pub fn non_null_type(&self) -> &Type {
        &self.non_null_type
    }

    pub fn comparable_type(&self) -> &Type {
        &self.comparable_type
    }

    // Specific class members

    pub fn listing_to_list_method(&self) -> NodeRef {
        self.listing_to_list_method
    }
    pub fn mapping_to_map_method(&self) -> NodeRef {
        self.mapping_to_map_method
    }

    pub fn list_join_method(&self) -> NodeRef {
        self.list_join_method
    }
    pub fn list_is_distinct_by_method(&self) -> NodeRef {
        self.list_is_distinct_by_method
    }
    pub fn list_fold_method(&self) -> NodeRef {
        self.list_fold_method
    }
    pub fn list_fold_indexed_method(&self) -> NodeRef {
        self.list_fold_indexed_method
    }
    pub fn list_is_empty_property(&self) -> NodeRef {
        self.list_is_empty_property
    }
    pub fn list_length_property(&self) -> NodeRef {
        self.list_length_property
    }
    pub fn list_is_distinct_property(&self) -> NodeRef {
        self.list_is_distinct_property
    }

    pub fn map_contains_key_method(&self) -> NodeRef {
        self.map_contains_key_method
    }
    pub fn map_get_or_null_method(&self) -> NodeRef {
        self.map_get_or_null_method
    }
    pub fn map_fold_method(&self) -> NodeRef {
        self.map_fold_method
    }
    pub fn map_is_empty_property(&self) -> NodeRef {
        self.map_is_empty_property
    }
    pub fn map_length_property(&self) -> NodeRef {
        self.map_length_property
    }
    pub fn map_keys_property(&self) -> NodeRef {
        self.map_keys_property
    }
}

fn find_class(
    tree: Arc<SyntaxTree>,
    classes: &[NodeId],
    name: &str,
) -> Result<Type, BaseModuleError> {
    let name = unquote_identifier(name);
    let Some(id) = classes.iter().find(|c| tree.name(**c) == Some(name)) else {
        return Err(BaseModuleError::MissingClass(name.to_string()));
    };
    Ok(Type::class(NodeRef {
        file_id: FileId::BASE,
        node_id: *id,
    }))
}

fn find_alias(
    tree: Arc<SyntaxTree>,
    aliases: &[NodeId],
    name: &str,
) -> Result<Type, BaseModuleError> {
    let name = unquote_identifier(name);
    let Some(id) = aliases.iter().find(|&c| tree.name(*c) == Some(name)) else {
        return Err(BaseModuleError::MissingAlias(name.to_string()));
    };

    Ok(Type::Alias {
        alias_node: NodeRef {
            file_id: FileId::BASE,
            node_id: *id,
        },
        type_args: vec![],
        constraints: vec![],
    })
}

fn find_class_member(
    tree: Arc<SyntaxTree>,
    classes: &[NodeId],
    class_name: &str,
    member_name: &str,
) -> Result<NodeRef, BaseModuleError> {
    let class_name = unquote_identifier(class_name);
    let member_name = unquote_identifier(member_name);
    let Some(class_id) = classes.iter().find(|c| tree.name(**c) == Some(class_name)) else {
        return Err(BaseModuleError::MissingClass(class_name.to_string()));
    };
    let member_parent = tree
        .child(*class_id, |k| matches!(k, NodeKind::ClassBody))
        .unwrap_or(*class_id);
    let member = tree
        .children_matching(member_parent, |k| {
            matches!(k, NodeKind::ClassProperty | NodeKind::ClassMethod)
        })
        .into_iter()
        .find(|n| tree.name(*n) == Some(member_name));
    let Some(member_id) = member else {
        return Err(BaseModuleError::MissingMember {
            class: class_name.to_string(),
            member: member_name.to_string(),
        });
    };
    Ok(NodeRef {
        file_id: FileId::BASE,
        node_id: member_id,
    })
}
