//! Shared utilities for `lsp-features` integration tests.

#![allow(dead_code)]

use std::collections::HashMap;

use pkl_syntax::ast::SyntaxTree;
use pkl_types::{AnalysisContext, BaseModule, FileId};

pub fn load_test_base() -> BaseModule {
    BaseModule::load_embedded().expect("embedded pkl:base must load")
}

/// Parse a snippet and run `f` with an `AnalysisContext`. Uses `FileId(1)`
/// with the embedded base tree pinned under its own file identity.
pub fn with_snippet<R>(
    base: &BaseModule,
    source: &str,
    f: impl FnOnce(&SyntaxTree, &AnalysisContext) -> R,
) -> R {
    let tree = std::sync::Arc::new(pkl_syntax::parse(source).expect("snippet should parse"));
    let trees = HashMap::from([
        (FileId(1), tree.clone()),
        (base.file_id(), base.tree().clone()),
    ]);
    let imports = HashMap::new();
    let ctx = AnalysisContext::new(base, &trees, &imports);
    f(&tree, &ctx)
}

pub fn offset(tree: &SyntaxTree, line: u32, column: u32) -> pkl_syntax::TextSize {
    tree.line_index()
        .offset(pkl_syntax::LineCol { line, col: column })
        .unwrap()
}

pub fn range(tree: &SyntaxTree, range: pkl_syntax::TextRange) -> lsp_types::Range {
    lsp_features::positions::PositionConverter::new(tree, Default::default()).range(range)
}
