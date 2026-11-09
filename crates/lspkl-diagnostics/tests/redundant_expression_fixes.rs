mod harness;

fn apply(source: &str, expected: &str, message: &str) {
    let base = harness::load_test_base();
    let file = base.add(0, source);
    let diagnostics = lspkl_diagnostics::diagnostics(&*base.db.borrow(), file);
    let diagnostic = diagnostics
        .iter()
        .find(|d| d.message == message)
        .unwrap_or_else(|| panic!("{source}: {diagnostics:?}"));
    let edits = &diagnostic.quick_fixes[0].edits;
    assert_eq!(edits.len(), 1);
    let edit = &edits[0];
    let mut edited = source.to_owned();
    edited.replace_range(
        usize::from(edit.span.start())..usize::from(edit.span.end()),
        &edit.new_text,
    );
    assert_eq!(edited, expected);
    let file = base.add(0, &edited);
    let after = lspkl_diagnostics::diagnostics(&*base.db.borrow(), file);
    assert!(
        !after.iter().any(|d| d.message == message),
        "{edited}: {after:?}"
    );
    assert!(after.is_empty(), "{edited}: {after:?}");
}

#[test]
fn redundant_type_cast_edits_preserve_precedence() {
    for (source, expected) in [
        ("x = 1 as Int", "x = 1"),
        ("x = 2 ** 3 as Number ** 4", "x = (2 ** 3) ** 4"),
        ("x = 1 + 2 as Int == 3", "x = 1 + 2 == 3"),
        (
            "x = 1 /* keep */ + 2 as Int * 3",
            "x = (1 /* keep */ + 2) * 3",
        ),
        ("x = (1 as Int) + 2", "x = (1) + 2"),
        ("x = 1 + 2 as Int * 3", "x = (1 + 2) * 3"),
        ("x = 1 + 2 as Int + 3", "x = 1 + 2 + 3"),
        ("x = 10 - 1 + 2 as Int", "x = 10 - 1 + 2"),
        ("x = (1 + 2) as Int", "x = (1 + 2)"),
    ] {
        apply(source, expected, "type cast is redundant");
    }
}

#[test]
fn redundant_coalescing_edits_preserve_nested_syntax() {
    for (source, expected) in [
        ("x = 1 ?? 2", "x = 1"),
        ("x = (1 + 2 ?? 3) * 4", "x = (1 + 2) * 4"),
        ("x = 1 ?? (2 ?? 3)", "x = 1"),
        ("x = (1 ?? 2).toString()", "x = (1).toString()"),
    ] {
        apply(source, expected, "null coalescing is redundant");
    }
}

#[test]
fn redundant_non_null_edits_preserve_nested_syntax() {
    for (source, expected) in [
        ("x = 1!!", "x = 1"),
        ("x = (1 + 2)!! * 3", "x = (1 + 2) * 3"),
        ("x = (1!!) + 2", "x = (1) + 2"),
        ("x = 1!!.toString()", "x = 1.toString()"),
    ] {
        apply(source, expected, "non null is redundant");
    }
}
