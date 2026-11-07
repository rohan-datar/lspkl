mod harness;

#[test]
fn explicit_generic_calls_and_self_types_diagnostics() {
    let base = harness::load_test_base();
    let file = base.add(
        1,
        include_str!("fixtures/diagnostics/inputs/pkl_033_generic_calls.pkl"),
    );
    let db = base.db.borrow();
    let ctx = lspkl_types::AnalysisContext::for_root(&*db, file);
    let diagnostics =
        lspkl_diagnostics::DiagnosticsEngine::with_default_analyzers().analyze(&ctx, file);
    let rendered =
        lspkl_diagnostics::render_diagnostics(&diagnostics, lspkl_db::line_index(&*db, file));
    insta::with_settings!({prepend_module_to_snapshot => false}, {
        insta::assert_snapshot!("snapshots__pkl_033_generic_calls_pkl", rendered);
    });
}
