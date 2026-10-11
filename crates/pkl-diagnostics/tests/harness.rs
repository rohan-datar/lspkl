//! Shared utilities for the diagnostics test suite.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use pkl_syntax::ast::SyntaxTree;
use pkl_types::{AnalysisContext, BaseModule, FileId, NodeRef};

pub fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .expect("CARGO_MANIFEST_DIR must be reachable")
}

/// Root of the diagnostic input fixtures (`.pkl` files exercising each
/// analyzer).
pub fn fixtures_root() -> PathBuf {
    crate_root().join("tests/fixtures/diagnostics/inputs")
}

pub fn load_test_base() -> BaseModule {
    BaseModule::load_embedded().expect("embedded pkl:base must load")
}

/// Recursively collect every `*.pkl` file under `root`, sorted for stable
/// snapshot ordering.
pub fn collect_pkl_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk(root, &mut files);
    files.sort();
    files
}

fn walk(root: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "pkl") {
            files.push(path);
        }
    }
}

/// Build an `AnalysisContext` over `tree`. Uses `FileId(1)` (won't collide
/// with `FileId::BASE`).
pub fn make_ctx<'a>(tree: &'a SyntaxTree, base: &'a BaseModule) -> AnalysisContext<'a> {
    AnalysisContext::new(tree, FileId(1), base, empty_trees(), empty_imports())
}

/// Shared empty registry for contexts returned by the test harness.
fn empty_trees() -> &'static HashMap<FileId, Arc<SyntaxTree>> {
    static TREES: LazyLock<HashMap<FileId, Arc<SyntaxTree>>> = LazyLock::new(HashMap::new);
    &TREES
}

fn empty_imports() -> &'static HashMap<NodeRef, pkl_types::ImportResolution> {
    static IMPORTS: LazyLock<HashMap<NodeRef, pkl_types::ImportResolution>> =
        LazyLock::new(HashMap::new);
    &IMPORTS
}
