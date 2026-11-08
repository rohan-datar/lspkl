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

#[test]
fn exact_class_values_cannot_downcast_but_class_variables_can() {
    let base = harness::load_test_base();
    let file = base.add(
        1,
        include_str!("fixtures/diagnostics/inputs/pkl033/exactClassExpressions.pkl"),
    );
    let db = base.db.borrow();
    let ctx = lspkl_types::AnalysisContext::for_root(&*db, file);
    let diagnostics = lspkl_diagnostics::DiagnosticsEngine::new()
        .with(Box::new(lspkl_diagnostics::analyzers::expr::ExprAnalyzer))
        .analyze(&ctx, file);
    let messages: Vec<_> = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert_eq!(
        messages,
        ["type cast cannot succeed", "expression is always false"]
    );
}

#[test]
fn module_class_values_remain_valid_class_generic_arguments() {
    let base = harness::load_test_base();
    let file = base.add(
        1,
        include_str!("fixtures/diagnostics/inputs/pkl033/moduleClassValues.pkl"),
    );
    let db = base.db.borrow();
    let ctx = lspkl_types::AnalysisContext::for_root(&*db, file);
    let diagnostics =
        lspkl_diagnostics::DiagnosticsEngine::with_default_analyzers().analyze(&ctx, file);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(
        diagnostics[0]
            .message
            .starts_with("Type mismatch. Expected: Class<Base>")
    );
    assert!(!diagnostics[0].message.contains("Class<>"));
}
