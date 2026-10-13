//! `Type::is_subtype_of` tests.

mod harness;

use pkl_types::Type;

#[test]
fn unknown_is_subtype_of_anything() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = pkl_types::AnalysisContext::new(&base, &*db, base.file_id());

    assert!(Type::Unknown.is_subtype_of(base.string_type(), &ctx));
    assert!(Type::Unknown.is_subtype_of(base.int_type(), &ctx));
    assert!(Type::Unknown.is_subtype_of(&Type::Nothing, &ctx));
}

#[test]
fn anything_is_subtype_of_unknown() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = pkl_types::AnalysisContext::new(&base, &*db, base.file_id());

    assert!(base.string_type().is_subtype_of(&Type::Unknown, &ctx));
    assert!(Type::Nothing.is_subtype_of(&Type::Unknown, &ctx));
}

#[test]
fn nothing_is_subtype_of_anything() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = pkl_types::AnalysisContext::new(&base, &*db, base.file_id());

    assert!(Type::Nothing.is_subtype_of(base.string_type(), &ctx));
    assert!(Type::Nothing.is_subtype_of(base.int_type(), &ctx));
}

#[test]
fn string_is_subtype_of_any_but_not_int() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = pkl_types::AnalysisContext::new(&base, &*db, base.file_id());

    assert!(base.string_type().is_subtype_of(base.any_type(), &ctx));
    assert!(!base.string_type().is_subtype_of(base.int_type(), &ctx));
}

#[test]
fn string_literal_is_subtype_of_string() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = pkl_types::AnalysisContext::new(&base, &*db, base.file_id());

    let lit = Type::string_literal("hello");
    assert!(lit.is_subtype_of(base.string_type(), &ctx));
    // Subtype of Any too (transitively).
    assert!(lit.is_subtype_of(base.any_type(), &ctx));
}

#[test]
fn union_subtype_iff_each_component_is() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = pkl_types::AnalysisContext::new(&base, &*db, base.file_id());

    // (Int | String) <: Any : both Int and String are subtypes of Any.
    let int_or_string = Type::union([base.int_type().clone(), base.string_type().clone()], &ctx);
    assert!(int_or_string.is_subtype_of(base.any_type(), &ctx));

    // (Int | String) is NOT a subtype of Int because String isn't.
    assert!(!int_or_string.is_subtype_of(base.int_type(), &ctx));
}

#[test]
fn member_is_subtype_of_a_union_containing_it() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = pkl_types::AnalysisContext::new(&base, &*db, base.file_id());

    let int_or_string = Type::union([base.int_type().clone(), base.string_type().clone()], &ctx);
    assert!(base.int_type().is_subtype_of(&int_or_string, &ctx));
    assert!(base.string_type().is_subtype_of(&int_or_string, &ctx));
    assert!(!base.boolean_type().is_subtype_of(&int_or_string, &ctx));
}

#[test]
fn type_is_subtype_of_itself() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = pkl_types::AnalysisContext::new(&base, &*db, base.file_id());

    assert!(base.string_type().is_subtype_of(base.string_type(), &ctx));
    assert!(base.int_type().is_subtype_of(base.int_type(), &ctx));
    assert!(Type::Unknown.is_subtype_of(&Type::Unknown, &ctx));
    assert!(Type::Nothing.is_subtype_of(&Type::Nothing, &ctx));
}

#[test]
fn string_literal_union_accepts_only_its_values() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = pkl_types::AnalysisContext::new(&base, &*db, base.file_id());
    let union = Type::union(
        [Type::string_literal("foo"), Type::string_literal("bar")],
        &ctx,
    );
    assert!(Type::string_literal("bar").is_subtype_of(&union, &ctx));
    assert!(!Type::string_literal("baz").is_subtype_of(&union, &ctx));
}
