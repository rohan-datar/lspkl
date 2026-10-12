use std::collections::HashMap;

use pkl_diagnostics::analyzers::module_uri::ModuleUriAnalyzer;
use pkl_diagnostics::diagnostic::QuickFixCommand;
use pkl_diagnostics::{Analyzer, Diagnostic, DiagnosticsHolder};
use pkl_syntax::ast::{NodeId, SyntaxTree};
use pkl_types::{AnalysisContext, BaseModule, FileId, ImportResolution, NodeRef};

fn owners(tree: &SyntaxTree, node: NodeId, found: &mut Vec<NodeId>) {
    if tree.kind(node).is_module_uri_owner() {
        found.push(node);
    }
    for &child in tree.children(node) {
        owners(tree, child, found);
    }
}

fn analyze(source: &str, resolution: Option<ImportResolution>) -> Vec<Diagnostic> {
    let tree = std::sync::Arc::new(pkl_syntax::parse(source).unwrap());
    let base = BaseModule::load_embedded().unwrap();
    let mut nodes = Vec::new();
    owners(&tree, tree.root(), &mut nodes);
    assert!(!nodes.is_empty());
    let imports = nodes
        .iter()
        .filter_map(|&node| {
            resolution
                .clone()
                .map(|edge| (NodeRef::new(FileId(1), node), edge))
        })
        .collect();
    let trees = HashMap::from([
        (FileId(1), tree.clone()),
        (base.file_id(), base.tree().clone()),
    ]);
    let ctx = AnalysisContext::new(&base, &trees, &imports);
    let mut holder = DiagnosticsHolder::new();
    for node in nodes {
        ModuleUriAnalyzer.analyze_node(NodeRef::new(FileId(1), node), &mut holder, &ctx);
    }
    holder.into_diagnostics()
}

#[test]
fn ordinary_extends_and_expression_failures_use_owner_edges() {
    for source in [
        r#"import "missing.pkl""#,
        r#"extends "missing.pkl""#,
        r#"value = import("missing.pkl")"#,
    ] {
        let diagnostics = analyze(
            source,
            Some(ImportResolution::Unresolved {
                package_to_download: None,
            }),
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].quick_fixes.is_empty());
        let span = diagnostics[0].span;
        assert_eq!(
            &source[std::ops::Range::<usize>::from(span)],
            "\"missing.pkl\""
        );
        assert!(
            analyze(
                source,
                Some(ImportResolution::Single(NodeRef::new(
                    FileId(2),
                    NodeId::ROOT
                )))
            )
            .is_empty()
        );
    }
    assert!(
        analyze(
            r#"import "modulepath:/valid.pkl""#,
            Some(ImportResolution::Single(NodeRef::new(
                FileId(2),
                NodeId::ROOT
            )))
        )
        .is_empty()
    );
    assert!(analyze(r#"import "missing.pkl""#, None).is_empty());
}

#[test]
fn empty_globs_warn_but_resolved_globs_do_not() {
    for source in [
        r#"import* "*.pkl" as modules"#,
        r#"value = import*("*.pkl")"#,
    ] {
        assert_eq!(
            analyze(source, Some(ImportResolution::Glob(vec![]))).len(),
            1
        );
        assert!(
            analyze(
                source,
                Some(ImportResolution::Glob(vec![NodeRef::new(
                    FileId(2),
                    NodeId::ROOT
                )]))
            )
            .is_empty()
        );
    }
    for uri in [
        "pkl:base",
        "http://example.test/*.pkl",
        "https://example.test/*.pkl",
        ".../*.pkl",
    ] {
        let source = format!("import* \"{uri}\" as modules");
        assert_eq!(
            analyze(&source, Some(ImportResolution::Glob(vec![]))).len(),
            1
        );
    }
}

#[test]
fn known_missing_package_has_download_command_unknown_dependency_does_not() {
    let package = "package://example.test/library@1.2.3";
    for reference in [
        "@library/main.pkl",
        "package://example.test/library@1.2.3#/main.pkl",
    ] {
        let source = format!("import \"{reference}\"");
        let diagnostics = analyze(
            &source,
            Some(ImportResolution::Unresolved {
                package_to_download: Some(package.into()),
            }),
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].quick_fixes.len(), 1);
        assert!(diagnostics[0].quick_fixes[0].edits.is_empty());
        assert_eq!(
            diagnostics[0].quick_fixes[0].command,
            Some(QuickFixCommand::DownloadPackage {
                uri: package.into()
            })
        );
    }
    let diagnostics = analyze(
        r#"import "@unknown/main.pkl""#,
        Some(ImportResolution::Unresolved {
            package_to_download: None,
        }),
    );
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].quick_fixes.is_empty());
}

#[test]
fn package_uri_validates_authority_path_semver_checksum_and_fragment() {
    for uri in [
        "package:/library@1.2.3#/main.pkl",
        "package://example.test",
        "package://example.test/@1.2.3#/main.pkl",
        "package://example.test/library#/main.pkl",
        "package://example.test/library@1#/main.pkl",
        "package://example.test/library@1.2.3::sha512:abc#/main.pkl",
        "package://example.test/library@1.2.3::sha256:#/main.pkl",
        "package://example.test/library@1.2.3::sha256:abc:extra#/main.pkl",
        "package://example.test/library@1.2.3",
        "package://example.test/library@1.2.3#main.pkl",
    ] {
        assert_eq!(
            analyze(&format!("import \"{uri}\""), None).len(),
            1,
            "{uri}"
        );
    }
    for uri in [
        "package://example.test/library@1.2.3#/main.pkl",
        "package://example.test/library@1.2.3-alpha.1+build::sha256:abc#/main.pkl",
    ] {
        assert!(
            analyze(&format!("import \"{uri}\""), None).is_empty(),
            "{uri}"
        );
    }
}
