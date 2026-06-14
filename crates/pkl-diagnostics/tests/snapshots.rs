//! Diagnostic snapshot tests.

mod harness;

use std::fs;

use pkl_diagnostics::{DiagnosticsEngine, render_diagnostics};

#[test]
fn diagnostic_snapshots() {
    let base = harness::load_test_base();
    let fixtures = harness::collect_pkl_files(&harness::fixtures_root());
    assert!(
        !fixtures.is_empty(),
        "no diagnostic fixtures found under {}",
        harness::fixtures_root().display()
    );

    let engine = DiagnosticsEngine::with_default_analyzers();

    for input_path in fixtures {
        let relative = input_path
            .strip_prefix(harness::fixtures_root())
            .expect("input path must be under fixtures root");

        let source = fs::read_to_string(&input_path).expect("fixture should be readable");
        let tree = pkl_syntax::parse(&source)
            .unwrap_or_else(|err| panic!("parse failed for {}: {err}", relative.display()));
        let ctx = harness::make_ctx(&tree, &base);

        let diagnostics = engine.analyze(&ctx);
        let rendered = render_diagnostics(&diagnostics);

        let snapshot_name = relative
            .display()
            .to_string()
            .replace(['/', '\\', '.'], "_");
        insta::assert_snapshot!(snapshot_name, rendered);
    }
}
