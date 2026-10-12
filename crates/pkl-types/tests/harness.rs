//! Shared test utilities for `pkl-types` integration tests.
//!
//! Two responsibilities:
//!   1. Load the embedded `pkl:base` used by production analysis.
//!   2. Provide a `find_first` helper for picking a specific node out of a
//!      parsed `SyntaxTree` — most tests want the first occurrence of a
//!      particular `NodeKind` and don't care about position.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};
use pkl_types::{AnalysisContext, BaseModule, FileId, NodeRef};

/// Single-file tests have no import edges.
pub fn empty_imports() -> &'static HashMap<NodeRef, pkl_types::ImportResolution> {
    static IMPORTS: LazyLock<HashMap<NodeRef, pkl_types::ImportResolution>> =
        LazyLock::new(HashMap::new);
    &IMPORTS
}

/// Load the same `pkl:base` source used by production analysis.
pub fn load_test_base() -> BaseModule {
    BaseModule::load_embedded().expect("embedded pkl:base must load")
}

/// Parse a small Pkl snippet and run `f` with an `AnalysisContext` over it.
/// Uses `FileId(1)` alongside the pinned base module.
pub fn with_snippet<R>(
    base: &BaseModule,
    source: &str,
    f: impl FnOnce(&AnalysisContext) -> R,
) -> R {
    let tree = Arc::new(pkl_syntax::parse(source).expect("snippet should parse"));
    let trees = pinned(base, tree);
    let ctx = AnalysisContext::new(base, &trees, empty_imports());
    f(&ctx)
}

/// Run `f` with an `AnalysisContext` whose user-tree is empty. Useful for
/// tests that exercise type operations over base-only types and don't need
/// any user-side AST.
pub fn with_ctx<R>(base: &BaseModule, f: impl FnOnce(&AnalysisContext) -> R) -> R {
    with_snippet(base, "", f)
}

/// Walk `tree` depth-first and return the first node whose kind satisfies
/// `predicate`. Panics if no such node exists.
pub fn find_first<F: Fn(&NodeKind) -> bool>(tree: &SyntaxTree, predicate: F) -> NodeId {
    fn go(tree: &SyntaxTree, id: NodeId, predicate: &impl Fn(&NodeKind) -> bool) -> Option<NodeId> {
        if predicate(tree.kind(id)) {
            return Some(id);
        }
        for &child in tree.children(id) {
            if let Some(hit) = go(tree, child, predicate) {
                return Some(hit);
            }
        }
        None
    }
    go(tree, tree.root(), &predicate).expect("expected node kind not found in tree")
}

pub fn pinned(base: &BaseModule, tree: Arc<SyntaxTree>) -> HashMap<FileId, Arc<SyntaxTree>> {
    HashMap::from([(FileId(1), tree), (base.file_id(), base.tree().clone())])
}

pub fn expr(
    node: NodeRef,
    ctx: &AnalysisContext,
) -> pkl_types::InFile<pkl_syntax::ast::nodes::Expr> {
    node.cast(ctx, pkl_syntax::ast::nodes::Expr::cast)
        .expect("expression")
}
