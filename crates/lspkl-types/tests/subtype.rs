//! `Type::is_subtype_of` tests.

mod harness;

use lspkl_types::Type;

#[test]
fn unknown_is_subtype_of_anything() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = lspkl_types::AnalysisContext::for_root(&*db, base.file_id());

    assert!(Type::Unknown.is_subtype_of(base.string_type(), ctx.db));
    assert!(Type::Unknown.is_subtype_of(base.int_type(), ctx.db));
    assert!(Type::Unknown.is_subtype_of(&Type::Nothing, ctx.db));
}

#[test]
fn anything_is_subtype_of_unknown() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = lspkl_types::AnalysisContext::for_root(&*db, base.file_id());

    assert!(base.string_type().is_subtype_of(&Type::Unknown, ctx.db));
    assert!(Type::Nothing.is_subtype_of(&Type::Unknown, ctx.db));
}

#[test]
fn nothing_is_subtype_of_anything() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = lspkl_types::AnalysisContext::for_root(&*db, base.file_id());

    assert!(Type::Nothing.is_subtype_of(base.string_type(), ctx.db));
    assert!(Type::Nothing.is_subtype_of(base.int_type(), ctx.db));
}

#[test]
fn string_is_subtype_of_any_but_not_int() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = lspkl_types::AnalysisContext::for_root(&*db, base.file_id());

    assert!(base.string_type().is_subtype_of(base.any_type(), ctx.db));
    assert!(!base.string_type().is_subtype_of(base.int_type(), ctx.db));
}

#[test]
fn string_literal_is_subtype_of_string() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = lspkl_types::AnalysisContext::for_root(&*db, base.file_id());

    let lit = Type::string_literal("hello");
    assert!(lit.is_subtype_of(base.string_type(), ctx.db));
    // Subtype of Any too (transitively).
    assert!(lit.is_subtype_of(base.any_type(), ctx.db));
}

#[test]
fn union_subtype_iff_each_component_is() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = lspkl_types::AnalysisContext::for_root(&*db, base.file_id());

    // (Int | String) <: Any : both Int and String are subtypes of Any.
    let int_or_string = Type::union(
        [base.int_type().clone(), base.string_type().clone()],
        ctx.db,
    );
    assert!(int_or_string.is_subtype_of(base.any_type(), ctx.db));

    // (Int | String) is NOT a subtype of Int because String isn't.
    assert!(!int_or_string.is_subtype_of(base.int_type(), ctx.db));
}

#[test]
fn member_is_subtype_of_a_union_containing_it() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = lspkl_types::AnalysisContext::for_root(&*db, base.file_id());

    let int_or_string = Type::union(
        [base.int_type().clone(), base.string_type().clone()],
        ctx.db,
    );
    assert!(base.int_type().is_subtype_of(&int_or_string, ctx.db));
    assert!(base.string_type().is_subtype_of(&int_or_string, ctx.db));
    assert!(!base.boolean_type().is_subtype_of(&int_or_string, ctx.db));
}

#[test]
fn type_is_subtype_of_itself() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = lspkl_types::AnalysisContext::for_root(&*db, base.file_id());

    assert!(base.string_type().is_subtype_of(base.string_type(), ctx.db));
    assert!(base.int_type().is_subtype_of(base.int_type(), ctx.db));
    assert!(Type::Unknown.is_subtype_of(&Type::Unknown, ctx.db));
    assert!(Type::Nothing.is_subtype_of(&Type::Nothing, ctx.db));
}

#[test]
fn string_literal_union_accepts_only_its_values() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = lspkl_types::AnalysisContext::for_root(&*db, base.file_id());
    let union = Type::union(
        [Type::string_literal("foo"), Type::string_literal("bar")],
        ctx.db,
    );
    assert!(Type::string_literal("bar").is_subtype_of(&union, ctx.db));
    assert!(!Type::string_literal("baz").is_subtype_of(&union, ctx.db));
}
