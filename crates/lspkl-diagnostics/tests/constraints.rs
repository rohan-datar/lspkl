mod harness;

fn messages(source: &str) -> Vec<String> {
    let base = harness::load_test_base();
    let file = base.add(1, source);
    let db = base.db.borrow();
    let ctx = lspkl_types::AnalysisContext::for_root(&*db, file);
    let diagnostics =
        lspkl_diagnostics::DiagnosticsEngine::with_default_analyzers().analyze(&ctx, file);
    diagnostics.iter().map(|d| d.message.clone()).collect()
}

#[test]
fn literal_constraints_report_only_definite_violations() {
    let actual = messages(
        r##"
        a: Int(this > 5) = 2
        b: Int(this > 5) = 6
        c: Int(this > 5) = read("prop:x").toInt()
        d: Int(this > 5 && unknownPredicate) = 6
        e: Boolean(this == true) = false
        f: Duration(this >= 1.s) = 500.ms
        g: DataSize(this >= 1.kb) = 500.b
        h: String(startsWith("a") && length >= 3) = "ab"
        i: String(matches(Regex(#"[0-9]+"#))) = "abc"
        j: Int(this > 5 || unknownPredicate) = 2
        weak: Int(this > 0) = 6
        narrowed: Int(this > 5) = weak
    "##,
    );
    let violated: Vec<_> = actual
        .iter()
        .filter(|m| m.starts_with("Type constraint"))
        .collect();
    assert_eq!(violated.len(), 6, "{actual:#?}");
    assert!(
        violated
            .iter()
            .any(|m| m.as_str() == "Type constraint `this > 5` violated.")
    );
    assert!(
        !actual.iter().any(|m| m.starts_with("Type mismatch")),
        "{actual:#?}"
    );
}

#[test]
fn alias_and_union_assignments_require_all_branches_to_fail() {
    let actual = messages(
        r#"
        typealias Small = Int(isBetween(0, 10))
        typealias Smaller = Small(this < 5)
        a: Smaller = 6
        b: UInt8 = 256
        c: Int16 = -32769
        d: UInt = -1
        e: Int(this < 0)|Int(this > 10) = 5
        f: Int(this < 0)|Int(this > 10) = 12
        g: Int(this < 0)|Int(unknownPredicate) = 5
        h: Int(this < 0)|String = 5
        i: (Int|Float)(this > 5) = 2
        j: Smaller = 4
        k: String(length > 2) = "hello"
    "#,
    );
    let violated: Vec<_> = actual
        .iter()
        .filter(|m| m.starts_with("Type constraint"))
        .collect();
    assert_eq!(violated.len(), 7, "{actual:#?}");
    assert!(
        !actual.iter().any(|m| m.starts_with("Type mismatch")),
        "{actual:#?}"
    );
}

#[test]
fn collection_sizes_and_string_literal_types_are_statically_known() {
    let actual = messages(
        r#"
        a: List(isEmpty) = List(1, 2)
        b: Set(length == 2) = Set(1, 1)
        c: Map(length == 2) = Map("key", 1, "key", 2)
        d: Set(length == 1) = Set(1, 1)
        e: Map(isEmpty) = Map()
        typealias Literal = "abc"
        literal: Literal = "abc"
        f: String(startsWith("z")) = literal
        g: Set(length == 1) = Set(read("prop:x"))
    "#,
    );
    let violated: Vec<_> = actual
        .iter()
        .filter(|m| m.starts_with("Type constraint"))
        .collect();
    assert_eq!(violated.len(), 4, "{actual:#?}");
    assert!(
        !actual.iter().any(|m| m.starts_with("Type mismatch")),
        "{actual:#?}"
    );
}

#[test]
fn transparent_parentheses_and_unary_minus_use_the_final_value_once() {
    let actual = messages(
        r#"
        a: Int(this > 0) = -(5)
        b: Int(this > 0) = (5)
        c: Int(this > 0) = ((-1))
        d: String(isEmpty) = ("abc")
        e: Int(this >= -9223372036854775808) = -9223372036854775808
    "#,
    );
    let violated: Vec<_> = actual
        .iter()
        .filter(|m| m.starts_with("Type constraint"))
        .collect();
    assert_eq!(violated.len(), 3, "{actual:#?}");
}

#[test]
fn shadowed_regex_constructor_is_undecidable() {
    let actual = messages(
        r#"
        function Regex(pattern: String): Regex = import("pkl:base").Regex(".*")
        value: String(matches(Regex("a"))) = "bbb"
        typealias Text = String(matches(Regex("a")))
        aliasValue: Text = "bbb"
    "#,
    );
    assert!(
        !actual.iter().any(|m| m.starts_with("Type constraint")),
        "{actual:#?}"
    );
}

#[test]
fn listing_elements_check_alias_carried_regex_constraints() {
    let actual = messages(
        r##"
        typealias Digits = String(matches(Regex(#"\d*"#)))
        aliasValue: Digits = "invalid"
        explicit = new Listing<Digits> { "invalid" }
        contextual: Listing<Digits> = new { "invalid" }
        satisfied: Listing<Digits> = new { "123" }
    "##,
    );
    let violated: Vec<_> = actual
        .iter()
        .filter(|m| m.starts_with("Type constraint"))
        .collect();
    assert_eq!(violated.len(), 3, "{actual:#?}");
}

#[test]
fn implicit_constraint_members_do_not_override_lexical_parameters() {
    let actual = messages(
        r#"
        function use(length: Int): String(length > 0) = ""
        function explicit(length: Int): String(this.length > 0) = ""
        function parity(isEven: Boolean): Int(isEven) = 3
        function explicitParity(isEven: Boolean): Int(this.isEven) = 3
    "#,
    );
    let violated: Vec<_> = actual
        .iter()
        .filter(|m| m.starts_with("Type constraint"))
        .collect();
    assert_eq!(violated.len(), 2, "{actual:#?}");
    assert!(violated.iter().all(|m| m.contains("this.")), "{actual:#?}");
}
