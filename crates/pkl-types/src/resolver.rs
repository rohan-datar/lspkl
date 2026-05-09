//! Name resolution and class hierarchy queries.

use pkl_analysis::ast::{NodeId, NodeKind, SyntaxTree};

use crate::context::AnalysisContext;
use crate::file_id::NodeRef;

/// Look up a class by simple name in `tree`. Searches only the module's
/// top-level `Class` declarations — no aliases, no imports, no nested scope.
///
/// Returns the `NodeId` of the matching `NodeKind::Class` node, or `None` if
/// no class with that name exists at module level.
///
/// To use across files, pair with [`AnalysisContext::tree_for`]:
///
/// ```ignore
/// let id = resolve_type_name_in_module(ctx.base.tree(), "Number")?;
/// let node_ref = NodeRef::new(ctx.base.file_id(), id);
/// ```
pub fn resolve_type_name_in_module(tree: &SyntaxTree, name: &str) -> Option<NodeId> {
    tree.children_matching(tree.root(), kind_is_class)
        .iter()
        .find(|class| tree.name(**class) == Some(name))
        .cloned()
}

/// Is class `a` a (non-strict) subclass of class `b`?
pub fn is_subclass(a: NodeRef, b: NodeRef, ctx: &AnalysisContext) -> bool {
    if a == b {
        return true;
    }

    let Some(a_tree) = ctx.tree_for(a.file_id) else {
        return false;
    };

    let Some(extends_target) = a_tree.class_extends_target(a.node_id) else {
        return false;
    };

    let Some(super_type_name) = a_tree.name(extends_target) else {
        return false;
    };

    let Some(super_type_node) = resolve_type_name_in_module(a_tree, super_type_name) else {
        return false;
    };

    is_subclass(
        NodeRef {
            file_id: a.file_id,
            node_id: super_type_node,
        },
        b,
        ctx,
    )
}

fn kind_is_class(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Class)
}
