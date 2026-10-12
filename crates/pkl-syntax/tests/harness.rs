#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};

/// Root of the `pkl-syntax` crate (where fixtures live, under `tests/snapshots/`).
pub fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .expect("crate root should be reachable")
}

pub fn parser_snapshot_input_dir() -> PathBuf {
    crate_root().join("tests/snapshots/parser/inputs")
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
    // Skip terminals and indent node-kind names by two spaces. Unmapped nodes
    // show their raw tree-sitter type so missing variants surface in snapshots.
    if matches!(tree.kind(node), NodeKind::Terminal(_)) {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(&"  ".repeat(depth));
    out.push_str(tree.kind(node).name());
    if let NodeKind::Unmapped(_) = tree.kind(node) {
        out.push_str(&format!(" /* unmapped: {} */", tree.kind(node).name()));
    }
    out.push('\n');
    for child in tree.children(node).iter().copied() {
        out.push_str(&render_ast_from_node(tree, child, depth + 1));
    }
    out
}
