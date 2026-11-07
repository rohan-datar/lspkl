mod harness;

#[test]
fn annotation_names_still_denote_instances() {
    let base = harness::load_test_base();
    base.add(
        1,
        "class Label extends Annotation {}\n@Label {}\nclass Foo {}\n",
    );
    let db = base.db.borrow();
    let file = lspkl_db::Db::file(&*db, &harness::uri(1)).unwrap();
    let ctx = lspkl_types::AnalysisContext::for_root(&*db, file);
    let diagnostics =
        lspkl_diagnostics::DiagnosticsEngine::with_default_analyzers().analyze(&ctx, file);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn class_values_diagnostics() {
    let base = harness::load_test_base();
    let source = include_str!("fixtures/diagnostics/inputs/classValues.pkl");
    base.add(1, source);
    let db = base.db.borrow();
    let ctx = lspkl_types::AnalysisContext::for_root(
        &*db,
        lspkl_db::Db::file(&*db, &harness::uri(1)).unwrap(),
    );
    let diagnostics =
        lspkl_diagnostics::DiagnosticsEngine::with_default_analyzers().analyze(&ctx, ctx.root);
    insta::assert_snapshot!(lspkl_diagnostics::render_diagnostics(
        &diagnostics,
        lspkl_db::line_index(ctx.db, ctx.root)
    ));
}
