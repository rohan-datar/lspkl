#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use pkl_analysis::ast::{NodeId, NodeKind, SyntaxTree};

/// Root of the `pkl-analysis` crate (where fixtures live, under `tests/snapshots/`).
pub fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .expect("crate root should be reachable")
}

pub fn parser_snapshot_input_dir() -> PathBuf {
    crate_root().join("tests/snapshots/parser/inputs")
}

/// Original pkl-lsp expected outputs. Useful for diffing against the
/// Rust output by hand; the Rust snapshots are managed by `insta`
/// separately.
pub fn parser_snapshot_output_dir() -> PathBuf {
    crate_root().join("tests/snapshots/parser/outputs")
}

pub fn diagnostics_input_dir() -> PathBuf {
    crate_root().join("tests/snapshots/diagnostics/inputs")
}

pub fn collect_pkl_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_pkl_files_recursive(root, &mut files);
    files.sort();
    files
}

pub fn render_ast(tree: &SyntaxTree) -> String {
    render_ast_from_node(tree, NodeId::ROOT, 0)
}

pub fn skip_if_missing_parser_fixtures() -> bool {
    let input_dir = parser_snapshot_input_dir();
    let output_dir = parser_snapshot_output_dir();
    !input_dir.exists() || !output_dir.exists()
}

pub fn skip_if_missing_diagnostics_inputs() -> bool {
    !diagnostics_input_dir().exists()
}

fn collect_pkl_files_recursive(root: &Path, files: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_pkl_files_recursive(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "pkl") {
            files.push(path);
        }
    }
}

fn render_ast_from_node(tree: &SyntaxTree, node: NodeId, depth: usize) -> String {
    // Mirror the Kotlin ParserSnippetTestEngine: skip Terminals, indent by 2, print
    // just the node-kind name. Unmapped nodes show their raw tree-sitter type so
    // missing variants surface in snapshot diffs.
    if matches!(tree.kind(node), NodeKind::Terminal(_)) {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(&"  ".repeat(depth));
    out.push_str(tree.kind(node).name());
    if let NodeKind::Unmapped(raw) = tree.kind(node) {
        out.push_str(&format!(" /* unmapped: {raw} */"));
    }
    out.push('\n');
    for child in tree.children(node).iter().copied() {
        out.push_str(&render_ast_from_node(tree, child, depth + 1));
    }
    out
}
