//! Shared utilities for `lsp-features` integration tests.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use pkl_syntax::ast::SyntaxTree;
use pkl_types::{AnalysisContext, BaseModule, FileId};

/// Path of the vendored `base.pkl` fixture (lives under pkl-types).
pub fn base_pkl_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .expect("CARGO_MANIFEST_DIR must be reachable")
        .join("../pkl-types/tests/fixtures/base.pkl")
}

pub fn load_test_base() -> BaseModule {
    let path = base_pkl_path();
    assert!(
        path.exists(),
        "base.pkl fixture missing at {}.\n\
         See crates/pkl-types/tests/fixtures/README.md.",
        path.display()
    );
    BaseModule::load_from_path(&path).expect("base.pkl should parse and resolve cleanly")
}

/// Parse a snippet and run `f` with an `AnalysisContext`. Uses `FileId(1)`
/// so it doesn't collide with `FileId::BASE` (which is 0).
pub fn with_snippet<R>(
    base: &BaseModule,
    source: &str,
    f: impl FnOnce(&SyntaxTree, &AnalysisContext) -> R,
) -> R {
    let tree = pkl_syntax::parse(source).expect("snippet should parse");
    let ctx = AnalysisContext::new(&tree, FileId(1), base);
    f(&tree, &ctx)
}
