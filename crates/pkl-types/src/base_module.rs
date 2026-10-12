//! The `pkl:base` standard library, loaded once at startup.

use std::sync::Arc;

use pkl_syntax::ast::nodes;
use pkl_syntax::ast::{NodeId, SyntaxTree, unquote_identifier};
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
        Self::from_tree(Arc::new(pkl_syntax::parse(source)?))
    }

    /// Load well-known metadata from an already parsed base module.
    pub fn from_tree(tree: Arc<SyntaxTree>) -> Result<Self, BaseModuleError> {
        let classes: Vec<_> = nodes::Module::cast(&tree, tree.root())
            .into_iter()
            .flat_map(|n| n.members(&tree))
            .filter(|&id| nodes::Class::cast(&tree, id).is_some())
            .collect();
        let aliases: Vec<_> = nodes::Module::cast(&tree, tree.root())
            .into_iter()
            .flat_map(|n| n.members(&tree))
            .filter(|&id| nodes::TypeAlias::cast(&tree, id).is_some())
            .collect();

        Ok(Self {
            tree: tree.clone(),
            file_id: FileId::BASE,

            // well-known types
            any_type: find_class(&tree, &classes, "Any")?,
            annotation_type: find_class(&tree, &classes, "Annotation")?,
            dynamic_type: find_class(&tree, &classes, "Dynamic")?,
            null_type: find_class(&tree, &classes, "Null")?,
            boolean_type: find_class(&tree, &classes, "Boolean")?,
            int_type: find_class(&tree, &classes, "Int")?,
            float_type: find_class(&tree, &classes, "Float")?,
            duration_type: find_class(&tree, &classes, "Duration")?,
            data_size_type: find_class(&tree, &classes, "DataSize")?,
            number_type: find_class(&tree, &classes, "Number")?,
            string_type: find_class(&tree, &classes, "String")?,
            bytes_type: find_class(&tree, &classes, "Bytes")?,
            collection_type: find_class(&tree, &classes, "Collection")?,
            listing_type: find_class(&tree, &classes, "Listing")?,
            list_type: find_class(&tree, &classes, "List")?,
            set_type: find_class(&tree, &classes, "Set")?,
            mapping_type: find_class(&tree, &classes, "Mapping")?,
            map_type: find_class(&tree, &classes, "Map")?,
            object_type: find_class(&tree, &classes, "Object")?,
            module_type: find_class(&tree, &classes, "Module")?,
            function_type: find_class(&tree, &classes, "Function")?,
            non_null_type: find_alias(&tree, &aliases, "NonNull")?,
            comparable_type: find_alias(&tree, &aliases, "Comparable")?,
            varargs_type: find_class(&tree, &classes, "VarArgs")?,

            // conversion methods
            listing_to_list_method: find_class_member(&tree, &classes, "Listing", "toList")?,
            mapping_to_map_method: find_class_member(&tree, &classes, "Mapping", "toMap")?,

            // List members duplicated on Listing
            list_join_method: find_class_member(&tree, &classes, "List", "join")?,
            list_is_distinct_by_method: find_class_member(&tree, &classes, "List", "isDistinctBy")?,
            list_fold_method: find_class_member(&tree, &classes, "List", "fold")?,
            list_fold_indexed_method: find_class_member(&tree, &classes, "List", "foldIndexed")?,
            list_is_empty_property: find_class_member(&tree, &classes, "List", "isEmpty")?,
            list_length_property: find_class_member(&tree, &classes, "List", "length")?,
            list_is_distinct_property: find_class_member(&tree, &classes, "List", "isDistinct")?,

            // Map members duplicated on Mapping
            map_contains_key_method: find_class_member(&tree, &classes, "Map", "containsKey")?,
            map_get_or_null_method: find_class_member(&tree, &classes, "Map", "getOrNull")?,
            map_fold_method: find_class_member(&tree, &classes, "Map", "fold")?,
            map_is_empty_property: find_class_member(&tree, &classes, "Map", "isEmpty")?,
            map_length_property: find_class_member(&tree, &classes, "Map", "length")?,
            map_keys_property: find_class_member(&tree, &classes, "Map", "keys")?,
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
        let fn_class_name = [
            "Function0",
            "Function1",
            "Function2",
            "Function3",
            "Function4",
            "Function5",
        ][n];
        let tree = self.tree.as_ref();
        let id = nodes::Module::cast(tree, tree.root())?
            .members(tree)
            .find(|&id| {
                nodes::Class::cast(tree, id).is_some() && tree.name(id) == Some(fn_class_name)
            })?;
        let classes = [id];
        let Ok(fn_class) = find_class(tree, &classes, fn_class_name) else {
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

fn find_class(tree: &SyntaxTree, classes: &[NodeId], name: &str) -> Result<Type, BaseModuleError> {
    let name = unquote_identifier(name);
    let Some(id) = classes.iter().find(|c| tree.name(**c) == Some(name)) else {
        return Err(BaseModuleError::MissingClass(name.to_string()));
    };
    Ok(Type::class(NodeRef {
        file: FileId::BASE,
        value: *id,
    }))
}

fn find_alias(tree: &SyntaxTree, aliases: &[NodeId], name: &str) -> Result<Type, BaseModuleError> {
    let name = unquote_identifier(name);
    let Some(id) = aliases.iter().find(|&c| tree.name(*c) == Some(name)) else {
        return Err(BaseModuleError::MissingAlias(name.to_string()));
    };

    Ok(Type::Alias {
        alias_node: NodeRef {
            file: FileId::BASE,
            value: *id,
        },
        type_args: vec![],
    })
}

fn find_class_member(
    tree: &SyntaxTree,
    classes: &[NodeId],
    class_name: &str,
    member_name: &str,
) -> Result<NodeRef, BaseModuleError> {
    let class_name = unquote_identifier(class_name);
    let member_name = unquote_identifier(member_name);
    let Some(class_id) = classes.iter().find(|c| tree.name(**c) == Some(class_name)) else {
        return Err(BaseModuleError::MissingClass(class_name.to_string()));
    };
    let member = nodes::Class::cast(tree, *class_id)
        .and_then(|n| n.body(tree))
        .and_then(|id| nodes::ClassBody::cast(tree, id))
        .into_iter()
        .flat_map(|n| n.members(tree))
        .find(|&n| tree.name(n) == Some(member_name));
    let Some(member_id) = member else {
        return Err(BaseModuleError::MissingMember {
            class: class_name.to_string(),
            member: member_name.to_string(),
        });
    };
    Ok(NodeRef {
        file: FileId::BASE,
        value: member_id,
    })
}
