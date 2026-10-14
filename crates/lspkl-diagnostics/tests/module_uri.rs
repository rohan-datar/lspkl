mod harness;
use lspkl_diagnostics::analyzers::module_uri::ModuleUriAnalyzer;
use lspkl_diagnostics::diagnostic::QuickFixCommand;
use lspkl_diagnostics::{Analyzer, Diagnostic, DiagnosticsHolder};
use lspkl_syntax::ast::{NodeId, SyntaxTree};
use lspkl_types::{AnalysisContext, NodeRef};

fn owners(tree: &SyntaxTree, node: NodeId, found: &mut Vec<NodeId>) {
    if tree.kind(node).is_module_uri_owner() {
        found.push(node);
    }
    for &child in tree.children(node) {
        owners(tree, child, found);
    }
}
fn analyze(source: &str, targets: &[(&str, &str)]) -> Vec<Diagnostic> {
    let base = harness::load_test_base();
    let project = harness::input_support::file_uri("PklProject");
    let projects = lspkl_project::PklProject::parse_metadata(&format!(
        r#"[{{"projectFileUri":"{project}","declaredDependencies":{{"library":"package://example.test/library@1.2.3"}}}}]"#
    ))
    .unwrap();
    base.db
        .borrow_mut()
        .set_projects(projects.into_iter().map(std::sync::Arc::new).collect());
    for &(name, text) in targets {
        base.add_path(name, text);
    }
    let file = base.add(1, source);
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, lspkl_db::Db::file(&*db, &harness::uri(1)).unwrap());
    let tree = ctx.tree(file).unwrap();
    let mut nodes = Vec::new();
    owners(tree, tree.root(), &mut nodes);
    assert!(!nodes.is_empty());
    let mut holder = DiagnosticsHolder::new();
    for node in nodes {
        ModuleUriAnalyzer.analyze_node(NodeRef::new(file, node), &mut holder, &ctx);
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
        let diagnostics = analyze(source, &[]);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].quick_fixes.is_empty());
        assert_eq!(
            &source[std::ops::Range::<usize>::from(diagnostics[0].span)],
            "\"missing.pkl\""
        );
        assert!(analyze(source, &[("missing.pkl", "value = 1")]).is_empty());
    }
}
#[test]
fn empty_globs_warn_but_resolved_globs_do_not() {
    for source in [
        r#"import* "target*.pkl" as modules"#,
        r#"value = import*("target*.pkl")"#,
    ] {
        let diagnostics = analyze(source, &[]);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "glob pattern has no matches");
        assert!(analyze(source, &[("target.pkl", "value = 1")]).is_empty());
    }
    for uri in [
        "pkl:base",
        "http://example.test/*.pkl",
        "https://example.test/*.pkl",
        ".../*.pkl",
    ] {
        assert_eq!(
            analyze(&format!("import* \"{uri}\" as modules"), &[]).len(),
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
        let diagnostics = analyze(&format!("import \"{reference}\""), &[]);
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
    let diagnostics = analyze(r#"import "@unknown/main.pkl""#, &[]);
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
        assert_eq!(analyze(&format!("import \"{uri}\""), &[]).len(), 1, "{uri}");
    }
    for uri in [
        "package://example.test/library@1.2.3#/main.pkl",
        "package://example.test/library@1.2.3-alpha.1+build::sha256:abc#/main.pkl",
    ] {
        assert!(
            analyze(&format!("import \"{uri}\""), &[])[0]
                .message
                .starts_with("missing package sources:"),
            "{uri}"
        );
    }
}
