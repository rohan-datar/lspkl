//! Diagnostic snapshot tests.

mod harness;

use std::fs;

use pkl_diagnostics::{DiagnosticsEngine, render_diagnostics};

#[test]
fn diagnostic_snapshots() {
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
        let tree = std::sync::Arc::new(
            pkl_syntax::parse(&source)
                .unwrap_or_else(|err| panic!("parse failed for {}: {err}", relative.display())),
        );
        let mut db = pkl_db::RootDatabase::default();
        let file = db
            .upsert_file(
                &pkl_vfs::path_to_uri(&input_path).unwrap(),
                std::sync::Arc::from(source.as_str()),
                None,
                pkl_db::FileStatus::Present,
            )
            .unwrap();
        harness::input_support::preload(&mut db);
        let ctx = pkl_types::AnalysisContext::for_root(&db, file);
        let diagnostics = engine.analyze(&ctx, file);
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
    let tree = std::sync::Arc::new(
        pkl_syntax::parse("import \"modulepath:/library.pkl\" as library\n").unwrap(),
    );
    base.add(1, tree.source());

    let db = base.db.borrow();

    let ctx = pkl_types::AnalysisContext::for_root(
        &*db,
        pkl_db::Db::file(&*db, &harness::uri(1)).unwrap(),
    );
    let diagnostics =
        DiagnosticsEngine::with_default_analyzers().analyze(&ctx, harness::file(&ctx, 1));
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("package")),
        "{diagnostics:?}"
    );
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
    let tree = std::sync::Arc::new(pkl_syntax::parse("value = (").unwrap());
    base.add(1, tree.source());

    let db = base.db.borrow();

    let ctx = pkl_types::AnalysisContext::for_root(
        &*db,
        pkl_db::Db::file(&*db, &harness::uri(1)).unwrap(),
    );
    let diagnostics =
        DiagnosticsEngine::with_default_analyzers().analyze(&ctx, harness::file(&ctx, 1));
    assert!(!diagnostics.is_empty());
    assert!(!render_diagnostics(&diagnostics, tree.line_index()).is_empty());
}
#[test]
fn analyzer_walk_keeps_file_identity_and_honors_subtree_skips() {
    use pkl_diagnostics::{Analyzer, DiagnosticsHolder};
    use pkl_syntax::ast::NodeKind;
    use pkl_types::{AnalysisContext, NodeRef};
    use std::sync::Arc;

    struct SkipClassBodies;
    impl Analyzer for SkipClassBodies {
        fn analyze_node(
            &self,
            node: NodeRef,
            holder: &mut DiagnosticsHolder,
            ctx: &AnalysisContext,
        ) -> bool {
            let tree = ctx.tree(node.file).unwrap();
            match tree.kind(node.value) {
                NodeKind::ClassBody => false,
                NodeKind::ClassProperty => {
                    holder.add_error(tree.span(node.value), tree.name(node.value).unwrap());
                    true
                }
                _ => true,
            }
        }
    }

    let base = harness::load_test_base();
    let first = Arc::new(pkl_syntax::parse("class C { inner = 1 }\ntopLevel = 2\n").unwrap());
    let second = Arc::new(pkl_syntax::parse("other = 3\n").unwrap());
    base.add(7, first.source());
    base.add(8, second.source());
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, pkl_db::Db::file(&*db, &harness::uri(7)).unwrap());
    let engine = DiagnosticsEngine::new().with(Box::new(SkipClassBodies));
    let first_diagnostics = engine.analyze(&ctx, harness::file(&ctx, 7));
    assert_eq!(first_diagnostics.len(), 1);
    assert_eq!(first_diagnostics[0].message, "topLevel");
    let property =
        first.children_matching(first.root(), |kind| matches!(kind, NodeKind::ClassProperty))[0];
    assert_eq!(first_diagnostics[0].span, first.span(property));
    let second_diagnostics = engine.analyze(&ctx, harness::file(&ctx, 8));
    assert_eq!(second_diagnostics.len(), 1);
    assert_eq!(second_diagnostics[0].message, "other");
    assert!(engine.analyze(&ctx, harness::file(&ctx, 99)).is_empty());
}

#[test]
fn constrained_casts_and_type_tests_are_not_reported_as_redundant() {
    let base = harness::load_test_base();
    let source = concat!(
        "value: Int = 1\n",
        "plainCast = value as Int\n",
        "constrainedCast = value as Int(isPositive)\n",
        "plainTest = value is Int\n",
        "constrainedTest = value is Int(isPositive)\n",
        "typealias Positive = Int(isPositive)\n",
        "aliasCast = value as Positive\n",
        "aliasTest = value is Positive\n",
        "unionCast = value as (Int(isPositive)|String)\n",
        "unionTest = value is (Int(isPositive)|String)\n",
        "typealias Choices = Int(isPositive)|String\n",
        "aliasUnionCast = value as Choices\n",
        "aliasUnionTest = value is Choices\n",
    );
    let tree = std::sync::Arc::new(pkl_syntax::parse(source).unwrap());
    base.add(1, tree.source());

    let db = base.db.borrow();

    let ctx = pkl_types::AnalysisContext::for_root(
        &*db,
        pkl_db::Db::file(&*db, &harness::uri(1)).unwrap(),
    );
    let diagnostics = DiagnosticsEngine::new()
        .with(Box::new(pkl_diagnostics::analyzers::expr::ExprAnalyzer))
        .analyze(&ctx, harness::file(&ctx, 1));
    let observed: Vec<_> = diagnostics
        .iter()
        .map(|diagnostic| {
            (
                tree.line_index().line_col(diagnostic.span.start()).line,
                diagnostic.message.as_str(),
            )
        })
        .collect();
    assert_eq!(
        observed,
        vec![
            (1, "type cast is redundant"),
            (3, "expression is always true")
        ]
    );
}

#[test]
fn let_binding_is_unresolved_in_its_initializer_but_visible_in_its_body() {
    let base = harness::load_test_base();
    let source = "result = let (value = value) value\n";
    let file = base.add(1, source);
    let db = base.db.borrow();
    let ctx = pkl_types::AnalysisContext::for_root(&*db, file);
    let diagnostics = DiagnosticsEngine::with_default_analyzers().analyze(&ctx, file);
    let unresolved: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.message.contains("unresolved reference"))
        .collect();
    assert_eq!(unresolved.len(), 1, "{diagnostics:?}");
    let initializer = source.find("= value").unwrap() + 2;
    assert_eq!(
        unresolved[0].span,
        pkl_syntax::TextRange::new(
            pkl_syntax::TextSize::from(initializer as u32),
            pkl_syntax::TextSize::from((initializer + "value".len()) as u32),
        ),
    );
}

#[test]
fn imported_modules_are_types_but_glob_imports_are_not() {
    let base = harness::load_test_base();
    base.add_path("k8s/api/v1/Probe.pkl", "port: Int = 80\n");
    base.add_path("k8s/api/v1/Port.pkl", "number: Int = 80\n");
    // The pkl-k8s layout: modules import siblings with upward `.../` searches.
    let source = "import \".../api/v1/Probe.pkl\" as ProbeModule\n\
                  import \".../api/v1/Port.pkl\"\n\
                  import* \"*.pkl\" as all\n\
                  probe: ProbeModule?\nports: Listing<Port>\nglobbed: all?\n";
    let file = base.add_path("k8s/api/v1/PodSpec.pkl", source);
    let db = base.db.borrow();
    let ctx = pkl_types::AnalysisContext::for_root(&*db, file);
    let diagnostics = DiagnosticsEngine::with_default_analyzers().analyze(&ctx, file);
    let unresolved: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.message.starts_with("unresolved reference"))
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert_eq!(unresolved, ["unresolved reference: all"], "{diagnostics:?}");
}
