mod harness;

use lspkl_diagnostics::{DiagnosticsEngine, render_diagnostics};
use lspkl_types::AnalysisContext;

#[test]
fn contextual_body_receivers_check_values_and_keep_dynamic_open() {
    let base = harness::load_test_base();
    let source = r#"
class Inner { number: Int = 42 }
class Outer { inner: Inner }
value: Outer = new { inner { number = "wrong" } }
listing: Listing<Int> = new { "wrong" }
mapping: Mapping<String, Int> = new { ["key"] = "wrong" }
dynamic = new Dynamic { arbitrary = 1; local value = arbitrary }
typealias Open = Dynamic
aliased: Open = new { arbitrary = 1; local value = arbitrary; local qualified = this.arbitrary }
"#;
    let file = base.add(1, source);
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, file);
    let diagnostics = DiagnosticsEngine::with_default_analyzers().analyze(&ctx, file);
    let rendered = render_diagnostics(&diagnostics, lspkl_db::line_index(&*db, file));
    assert_eq!(diagnostics.len(), 3, "{rendered}");
    assert!(
        diagnostics
            .iter()
            .all(|d| d.message.starts_with("Type mismatch. Expected: Int.")),
        "{rendered}"
    );
}

#[test]
fn nested_output_and_generator_bodies_have_no_false_positives() {
    let base = harness::load_test_base();
    let source = r#"
output { files { ["hello.txt"] { text = "hello" } } }
x: Listing<Int> = new {
  when (true) { 1 }
  for (n in List(1, 2)) { n }
}
"#;
    let file = base.add(1, source);
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, file);
    let diagnostics = DiagnosticsEngine::with_default_analyzers().analyze(&ctx, file);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

/// Set LSPKL_DIAGNOSTIC_AUDIT to retain a full, stable diagnostic listing for
/// comparing semantic changes against the embedded SDK and fixture corpus.
#[test]
#[ignore = "explicit broad diagnostic audit; writes the requested report"]
fn audit_sdk_and_fixture_diagnostics() {
    use fluent_uri::Uri;
    use lspkl_db::Db;
    let mut report = String::new();
    let engine = DiagnosticsEngine::with_default_analyzers();
    for name in lspkl_vfs::stdlib::MODULE_NAMES {
        let mut db = lspkl_db::RootDatabase::default();
        let file = db
            .file(&Uri::parse(format!("pkl:{name}")).unwrap())
            .unwrap();
        harness::input_support::preload(&mut db);
        let ctx = AnalysisContext::for_root(&db, file);
        let diagnostics = engine.analyze(&ctx, file);
        report.push_str(&format!("=== pkl:{name} ===\n"));
        report.push_str(&render_diagnostics(
            &diagnostics,
            lspkl_db::line_index(&db, file),
        ));
    }
    for path in harness::collect_pkl_files(&harness::fixtures_root()) {
        let mut db = lspkl_db::RootDatabase::default();
        let file = db
            .upsert_file(
                &lspkl_vfs::path_to_uri(&path).unwrap(),
                std::sync::Arc::from(std::fs::read_to_string(&path).unwrap()),
                None,
                lspkl_db::FileStatus::Present,
            )
            .unwrap();
        harness::input_support::preload(&mut db);
        let ctx = AnalysisContext::for_root(&db, file);
        let diagnostics = engine.analyze(&ctx, file);
        report.push_str(&format!(
            "=== {} ===\n",
            path.strip_prefix(harness::fixtures_root())
                .unwrap()
                .display()
        ));
        report.push_str(&render_diagnostics(
            &diagnostics,
            lspkl_db::line_index(&db, file),
        ));
    }
    let path =
        std::env::var("LSPKL_DIAGNOSTIC_AUDIT").expect("set LSPKL_DIAGNOSTIC_AUDIT to report path");
    std::fs::write(path, report).unwrap();
}

#[test]
fn contextual_receivers_accept_any_values_and_valid_spread_sources() {
    let base = harness::load_test_base();
    let source = r#"
class Report { number: Int = 42 }
output { value = new Report {} }
ints: Listing<Int> = new { ...Bytes(1, 2); ...List(3, 4) }
optional: Listing<Int>? = null
safe: Listing<Int> = new { ...?optional }
"#;
    let file = base.add(1, source);
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, file);
    let diagnostics = DiagnosticsEngine::with_default_analyzers().analyze(&ctx, file);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn receiver_guards_do_not_leak_into_false_branches_or_shadowed_bindings() {
    let base = harness::load_test_base();
    let source = r#"
open class Parent {}
class Child extends Parent { number: Int }
value: Parent = new Child {}
good = if (value is Child) value.number else 0
bad = if (value is Child) 0 else value.number
shadowed = if (value is Child) let (value = new Parent {}) value.number else 0
"#;
    let file = base.add(1, source);
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, file);
    let diagnostics = DiagnosticsEngine::with_default_analyzers().analyze(&ctx, file);
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    assert!(
        diagnostics
            .iter()
            .all(|d| d.message == "unresolved reference: number"),
        "{diagnostics:?}"
    );
}

/// `[[condition]]` binds `this` to each member's value, not the enclosing
/// object, so tests on it are neither always true nor always false.
#[test]
fn member_predicate_conditions_use_member_values_as_this() {
    let base = harness::load_test_base();
    let source = r#"
open class Base
class Sub extends Base
dynamic = new Dynamic { [[this is this]] = 5; [[this is module]] = 6 }
typed: Listing<Base> = new { new Sub {}; [[this is Sub]] { } }
numbers: Listing<Int> = new { 1; [[this > 0]] = 2 }
"#;
    let file = base.add(1, source);
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, file);
    let diagnostics = DiagnosticsEngine::with_default_analyzers().analyze(&ctx, file);
    let rendered = render_diagnostics(&diagnostics, lspkl_db::line_index(&*db, file));
    assert!(diagnostics.is_empty(), "{rendered}");
}
