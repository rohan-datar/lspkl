//! `Type::render` tests.

mod harness;

use pkl_types::Type;

#[test]
fn class_renders_to_its_simple_name() {
    let base = harness::load_test_base();
    harness::with_ctx(&base, |ctx| {
        assert_eq!(base.string_type().render(ctx.db), "String");
        assert_eq!(base.int_type().render(ctx.db), "Int");
    });
}

#[test]
fn string_literal_renders_with_quotes() {
    let base = harness::load_test_base();
    harness::with_ctx(&base, |ctx| {
        assert_eq!(Type::string_literal("hello").render(ctx.db), "\"hello\"");
    });
}

#[test]
fn union_renders_with_pipe() {
    let base = harness::load_test_base();
    harness::with_ctx(&base, |ctx| {
        let union = Type::union(
            [base.int_type().clone(), base.string_type().clone()],
            ctx.db,
        );
        assert_eq!(union.render(ctx.db), "Int|String");
    });
}

#[test]
fn nullable_renders_with_question_mark() {
    // Pkl's `T?` desugars to `Union(Null, T)`. The renderer should reverse
    // that: render `Union(Null, T)` and `Union(T, Null)` as `T?`.
    let base = harness::load_test_base();
    harness::with_ctx(&base, |ctx| {
        let nullable_string = Type::union(
            [base.null_type().clone(), base.string_type().clone()],
            ctx.db,
        );
        assert_eq!(nullable_string.render(ctx.db), "String?");

        let string_or_null = Type::union(
            [base.string_type().clone(), base.null_type().clone()],
            ctx.db,
        );
        assert_eq!(string_or_null.render(ctx.db), "String?");
    });
}
