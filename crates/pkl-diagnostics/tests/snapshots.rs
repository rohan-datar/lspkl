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
        let rendered = render_diagnostics(&diagnostics, tree.line_index());

        let snapshot_name = relative
            .display()
            .to_string()
            .replace(['/', '\\', '.'], "_");
        insta::assert_snapshot!(snapshot_name, rendered);
    }
}

#[test]
fn modulepath_imports_do_not_use_package_uri_rules() {
    let base = harness::load_test_base();
    let tree = pkl_syntax::parse("import \"modulepath:/library.pkl\" as library\n").unwrap();
    let ctx = harness::make_ctx(&tree, &base);
    let diagnostics = DiagnosticsEngine::with_default_analyzers().analyze(&ctx);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn renderer_keeps_one_based_byte_columns_and_edit_endpoints() {
    use pkl_diagnostics::{Diagnostic, QuickFix, TextEdit};
    use pkl_syntax::{LineIndex, TextRange, TextSize};

    let source = "é😀 x\r\ny = 1\n";
    let index = LineIndex::new(source);
    let range = TextRange::new(TextSize::from(7), TextSize::from(8));
    let before = TextEdit::insert_before(range, "before");
    let after = TextEdit::insert_after(range, "after");
    assert_eq!(before.span, TextRange::empty(range.start()));
    assert_eq!(after.span, TextRange::empty(range.end()));
    assert!(before.is_insertion());
    assert!(after.is_insertion());
    let diagnostic = Diagnostic::error(range, "example").with_fix(QuickFix::new(
        "fix",
        vec![before, after, TextEdit::replace(range, "replacement")],
    ));
    assert_eq!(
        render_diagnostics(&[diagnostic], &index),
        concat!(
            "1:8 error: example\n",
            "  fix: fix\n",
            "    insert \"before\" at 1:8\n",
            "    insert \"after\" at 1:9\n",
            "    replace 1:8-1:9 with \"replacement\"\n",
        )
    );
    assert_eq!(render_diagnostics(&[], &index), "(no diagnostics)\n");
}

#[test]
fn malformed_source_remains_analyzable_at_eof() {
    let base = harness::load_test_base();
    let tree = pkl_syntax::parse("value = (").unwrap();
    let ctx = harness::make_ctx(&tree, &base);
    let diagnostics = DiagnosticsEngine::with_default_analyzers().analyze(&ctx);
    assert!(!diagnostics.is_empty());
    assert!(!render_diagnostics(&diagnostics, tree.line_index()).is_empty());
}
