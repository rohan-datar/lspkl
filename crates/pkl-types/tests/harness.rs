//! Shared test utilities for `pkl-types` integration tests.
//!
//! Two responsibilities:
//!   1. Locate and load the vendored `base.pkl` fixture (see `fixtures/README.md`).
//!   2. Provide a `find_first` helper for picking a specific node out of a
//!      parsed `SyntaxTree` — most tests want the first occurrence of a
//!      particular `NodeKind` and don't care about position.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use pkl_analysis::ast::{NodeId, NodeKind, SyntaxTree};
use pkl_types::{AnalysisContext, BaseModule, FileId};

/// Path of the vendored `base.pkl` fixture. See `tests/fixtures/README.md`.
pub fn base_pkl_path() -> PathBuf {
    crate_root().join("tests/fixtures/base.pkl")
}

pub fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .expect("CARGO_MANIFEST_DIR must be reachable")
}

/// Load the vendored `base.pkl` and return a `BaseModule`. Panics with a
/// clear message if the fixture isn't present — that's the TDD signal to
/// vendor it.
pub fn load_test_base() -> BaseModule {
    let path = base_pkl_path();
    assert!(
        path.exists(),
        "base.pkl fixture missing.\n\
         Drop the canonical pkl:base source at:\n  {}",
        path.display()
    );
    BaseModule::load_from_path(&path).expect("base.pkl should parse and resolve cleanly")
}

/// Parse a small Pkl snippet and run `f` with an `AnalysisContext` over it.
/// Uses `FileId(1)` so it doesn't collide with [`FileId::BASE`] (which is 0).
pub fn with_snippet<R>(
    base: &BaseModule,
    source: &str,
    f: impl FnOnce(&AnalysisContext) -> R,
) -> R {
    let tree = pkl_analysis::parse(source).expect("snippet should parse");
    let ctx = AnalysisContext::new(&tree, FileId(1), base);
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
