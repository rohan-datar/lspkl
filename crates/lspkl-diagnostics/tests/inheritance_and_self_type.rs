mod harness;

use lspkl_diagnostics::DiagnosticsEngine;
use lspkl_diagnostics::analyzers::{MemberAnalyzer, SelfTypeAnalyzer};
use lspkl_types::AnalysisContext;

/// The analyzers that own the inheritance, const-override and self-type rules.
fn engine() -> DiagnosticsEngine {
    DiagnosticsEngine::new()
        .with(Box::new(MemberAnalyzer))
        .with(Box::new(SelfTypeAnalyzer))
}

fn analyze(source: &str) -> Vec<lspkl_diagnostics::Diagnostic> {
    let base = harness::load_test_base();
    let file = base.add(1, source);
    let db = base.db.borrow();
    engine().analyze(&AnalysisContext::for_root(&*db, file), file)
}

#[test]
fn pkl_033_rules_apply_without_version_but_respect_explicit_older_target() {
    let body = "class C { abstract function f(): Int }\ntypealias Bad = this\n";
    assert_eq!(analyze(body).len(), 2);
    for version in ["0.32.0", "0.32", "0.31.0"] {
        assert!(
            analyze(&format!(
                "@ModuleInfo {{ minPklVersion = \"{version}\" }}\nmodule Example\n{body}"
            ))
            .is_empty()
        );
    }
    assert_eq!(
        analyze(&format!(
            "@ModuleInfo {{ minPklVersion = \"0.33.0\" }}\nmodule Example\n{body}"
        ))
        .len(),
        2
    );
}

#[test]
fn inherited_abstract_methods_cross_files_and_sdk() {
    let base = harness::load_test_base();
    base.add_path("Parent.pkl", "abstract module Parent\nabstract function f(): Int\nabstract class A { abstract function g(): String }\n");
    let file = base.add_path("Child.pkl", "module Child\nextends \"Parent.pkl\"\nimport \"Parent.pkl\"\nimport \"pkl:reflect\"\nclass Missing extends Parent.A {}\nclass Done extends Parent.A { function g() = \"ok\" }\n");
    let db = base.db.borrow();
    let diagnostics = engine().analyze(&AnalysisContext::for_root(&*db, file), file);
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("method `f()`"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("method `g()`"))
    );
}

#[test]
fn embedded_sdk_has_no_inheritance_or_self_type_diagnostics() {
    use lspkl_db::Db;
    let base = harness::load_test_base();
    let mut db = base.db.borrow_mut();
    let mut modules = Vec::new();
    for name in lspkl_vfs::stdlib::MODULE_NAMES {
        modules.push(
            db.file(&fluent_uri::Uri::parse(format!("pkl:{name}")).unwrap())
                .unwrap(),
        );
    }
    harness::input_support::preload(&mut db);
    let engine = engine();
    for file in modules {
        let diagnostics = engine.analyze(&AnalysisContext::for_root(&*db, file), file);
        assert!(
            diagnostics.is_empty(),
            "{}: {diagnostics:?}",
            file.uri(&*db)
        );
    }
}

#[test]
fn self_type_scopes_distinguish_constraints_objects_and_const_members() {
    assert!(analyze("typealias Good = Int(this is Int)\nconst good: Int(this is Int) = 1\nconst nested = new Dynamic { local value: this? = null }\n").is_empty());
    assert_eq!(analyze("typealias Bad = this(true)\nconst bad: this(true) = null\nconst nested = new Dynamic { local const bad: this? = null }\n").len(), 3);
}

#[test]
fn const_assignments_cover_sdk_amendments_and_class_instances() {
    let source = include_str!("fixtures/diagnostics/inputs/pkl033/constAssignment.pkl");
    let diagnostics = analyze(source);
    assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message == "Cannot amend const property `current`.")
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message == "Cannot assign to const property `pi`.")
    );
}

#[test]
fn local_methods_do_not_implement_inherited_abstract_methods() {
    let diagnostics = analyze(
        "abstract class A { abstract function f(): Int }\nclass B extends A { local function f(): Int = 1 }\n",
    );
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(diagnostics[0].message.contains("implement method `f()`"));
}

#[test]
fn amending_module_cannot_assign_inherited_const_properties() {
    let base = harness::load_test_base();
    base.add_path("Parent.pkl", "open module Parent\nconst value: Int = 1\n");
    let file = base.add_path("Child.pkl", "amends \"Parent.pkl\"\nvalue = 2\n");
    let db = base.db.borrow();
    let diagnostics = engine().analyze(&AnalysisContext::for_root(&*db, file), file);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(
        diagnostics[0].message,
        "Cannot assign to const property `value`."
    );
}

#[test]
fn upstream_this_type_const_depth_cases() {
    assert!(
        analyze(include_str!(
            "fixtures/diagnostics/inputs/pkl033/thisType9.pkl"
        ))
        .is_empty()
    );
    let key = analyze(include_str!(
        "fixtures/diagnostics/inputs/pkl033/thisType10.pkl"
    ));
    assert_eq!(key.len(), 1, "{key:?}");
    assert_eq!(
        key[0].message,
        "Cannot reference `this` type from const method `foo`."
    );
    let lambda = analyze(include_str!(
        "fixtures/diagnostics/inputs/pkl033/thisType11.pkl"
    ));
    assert_eq!(lambda.len(), 1, "{lambda:?}");
    assert_eq!(
        lambda[0].message,
        "Cannot reference `this` type from const method `func.<function#1>`."
    );
    assert!(
        analyze(include_str!(
            "fixtures/diagnostics/inputs/pkl033/memberPredicateThis.pkl"
        ))
        .is_empty()
    );
}

#[test]
fn nested_amendments_resolve_property_receivers_conservatively() {
    let diagnostics = analyze(include_str!(
        "fixtures/diagnostics/inputs/pkl033/nestedConstAssignment.pkl"
    ));
    assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.message == "Cannot assign to const property `secret`.")
    );
}

#[test]
fn abstract_method_obligations_include_parameter_names() {
    let diagnostics = analyze(include_str!(
        "fixtures/diagnostics/inputs/pkl033/abstractMethodParameters.pkl"
    ));
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(
        diagnostics[0]
            .message
            .contains("method `foo(first, second)`")
    );
}

/// Assigning a const property was already an error before Pkl 0.33, so the
/// check is not version-gated.
#[test]
fn const_assignments_apply_regardless_of_declared_version() {
    let body = "class C { const x: Int = 1 }\nc = new C { x = 2 }\n";
    for header in [
        "",
        "@ModuleInfo { minPklVersion = \"0.31.0\" }\nmodule Example\n",
    ] {
        let diagnostics = analyze(&format!("{header}{body}"));
        assert_eq!(diagnostics.len(), 1, "{header}: {diagnostics:?}");
        assert_eq!(
            diagnostics[0].message,
            "Cannot assign to const property `x`."
        );
    }
}
