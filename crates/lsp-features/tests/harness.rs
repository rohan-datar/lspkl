//! Shared utilities for `lsp-features` integration tests.

#![allow(dead_code)]

use std::collections::HashMap;

use pkl_syntax::ast::SyntaxTree;
use pkl_types::{AnalysisContext, BaseModule, FileId};

pub fn load_test_base() -> BaseModule {
    BaseModule::load_embedded().expect("embedded pkl:base must load")
}

/// Parse a snippet and run `f` with an `AnalysisContext`. Uses `FileId(1)`
/// so it doesn't collide with `FileId::BASE` (which is 0).
pub fn with_snippet<R>(
    base: &BaseModule,
    source: &str,
    f: impl FnOnce(&SyntaxTree, &AnalysisContext) -> R,
) -> R {
    let tree = pkl_syntax::parse(source).expect("snippet should parse");
    let trees = HashMap::new();
    let imports = HashMap::new();
    let ctx = AnalysisContext::new(&tree, FileId(1), base, &trees, &imports);
    f(&tree, &ctx)
}
