//! Shared utilities for the diagnostics test suite.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pkl_syntax::ast::SyntaxTree;
use pkl_types::{BaseModule, FileId};

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

/// Pin the analyzed document and embedded base tree for an analysis context.
pub fn pinned_trees(tree: &Arc<SyntaxTree>, base: &BaseModule) -> HashMap<FileId, Arc<SyntaxTree>> {
    HashMap::from([
        (FileId(1), tree.clone()),
        (base.file_id(), base.tree().clone()),
    ])
}
