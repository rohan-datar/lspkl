//! Shared utilities for the diagnostics test suite.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use pkl_syntax::ast::SyntaxTree;
use pkl_types::{AnalysisContext, BaseModule, FileId};

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

pub fn base_pkl_path() -> PathBuf {
    crate_root().join("../pkl-types/tests/fixtures/base.pkl")
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
    AnalysisContext::new(tree, FileId(1), base)
}
