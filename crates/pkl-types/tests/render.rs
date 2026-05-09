//! `Type::render` tests.

mod harness;

use pkl_types::Type;

#[test]
fn unknown_renders() {
    let base = harness::load_test_base();
    harness::with_ctx(&base, |ctx| {
        assert_eq!(Type::Unknown.render(ctx), "unknown");
    });
}

#[test]
fn nothing_renders() {
    let base = harness::load_test_base();
    harness::with_ctx(&base, |ctx| {
        assert_eq!(Type::Nothing.render(ctx), "nothing");
    });
}

#[test]
fn class_renders_to_its_simple_name() {
    let base = harness::load_test_base();
    harness::with_ctx(&base, |ctx| {
        assert_eq!(base.string_type().render(ctx), "String");
        assert_eq!(base.int_type().render(ctx), "Int");
    });
}

#[test]
fn string_literal_renders_with_quotes() {
    let base = harness::load_test_base();
    harness::with_ctx(&base, |ctx| {
        assert_eq!(Type::string_literal("hello").render(ctx), "\"hello\"");
    });
}

#[test]
fn union_renders_with_pipe() {
    let base = harness::load_test_base();
    harness::with_ctx(&base, |ctx| {
        let union = Type::Union {
            left: Box::new(base.int_type().clone()),
            right: Box::new(base.string_type().clone()),
            constraints: vec![],
        };
        assert_eq!(union.render(ctx), "Int|String");
    });
}

#[test]
fn nullable_renders_with_question_mark() {
    // Pkl's `T?` desugars to `Union(Null, T)`. The renderer should reverse
    // that: render `Union(Null, T)` and `Union(T, Null)` as `T?`.
    let base = harness::load_test_base();
    harness::with_ctx(&base, |ctx| {
        let nullable_string = Type::Union {
            left: Box::new(base.null_type().clone()),
            right: Box::new(base.string_type().clone()),
            constraints: vec![],
        };
        assert_eq!(nullable_string.render(ctx), "String?");

        let string_or_null = Type::Union {
            left: Box::new(base.string_type().clone()),
            right: Box::new(base.null_type().clone()),
            constraints: vec![],
        };
        assert_eq!(string_or_null.render(ctx), "String?");
    });
}
