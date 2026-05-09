//! `Type::is_subtype_of` tests.

mod harness;

use pkl_types::{AnalysisContext, FileId, Type};

fn ctx<'a>(
    base: &'a pkl_types::BaseModule,
    dummy: &'a pkl_analysis::ast::SyntaxTree,
) -> AnalysisContext<'a> {
    AnalysisContext::new(dummy, FileId(1), base)
}

#[test]
fn unknown_is_subtype_of_anything() {
    let base = harness::load_test_base();
    let dummy = pkl_analysis::parse("").unwrap();
    let ctx = ctx(&base, &dummy);

    assert!(Type::Unknown.is_subtype_of(base.string_type(), &ctx));
    assert!(Type::Unknown.is_subtype_of(base.int_type(), &ctx));
    assert!(Type::Unknown.is_subtype_of(&Type::Nothing, &ctx));
}

#[test]
fn anything_is_subtype_of_unknown() {
    let base = harness::load_test_base();
    let dummy = pkl_analysis::parse("").unwrap();
    let ctx = ctx(&base, &dummy);

    assert!(base.string_type().is_subtype_of(&Type::Unknown, &ctx));
    assert!(Type::Nothing.is_subtype_of(&Type::Unknown, &ctx));
}

#[test]
fn nothing_is_subtype_of_anything() {
    let base = harness::load_test_base();
    let dummy = pkl_analysis::parse("").unwrap();
    let ctx = ctx(&base, &dummy);

    assert!(Type::Nothing.is_subtype_of(base.string_type(), &ctx));
    assert!(Type::Nothing.is_subtype_of(base.int_type(), &ctx));
}

#[test]
fn string_is_subtype_of_any_but_not_int() {
    let base = harness::load_test_base();
    let dummy = pkl_analysis::parse("").unwrap();
    let ctx = ctx(&base, &dummy);

    assert!(base.string_type().is_subtype_of(base.any_type(), &ctx));
    assert!(!base.string_type().is_subtype_of(base.int_type(), &ctx));
}

#[test]
fn string_literal_is_subtype_of_string() {
    let base = harness::load_test_base();
    let dummy = pkl_analysis::parse("").unwrap();
    let ctx = ctx(&base, &dummy);

    let lit = Type::string_literal("hello");
    assert!(lit.is_subtype_of(base.string_type(), &ctx));
    // Subtype of Any too (transitively).
    assert!(lit.is_subtype_of(base.any_type(), &ctx));
}

#[test]
fn union_subtype_iff_each_component_is() {
    let base = harness::load_test_base();
    let dummy = pkl_analysis::parse("").unwrap();
    let ctx = ctx(&base, &dummy);

    // (Int | String) <: Any : both Int and String are subtypes of Any.
    let int_or_string = Type::Union {
        left: Box::new(base.int_type().clone()),
        right: Box::new(base.string_type().clone()),
        constraints: vec![],
    };
    assert!(int_or_string.is_subtype_of(base.any_type(), &ctx));

    // (Int | String) is NOT a subtype of Int because String isn't.
    assert!(!int_or_string.is_subtype_of(base.int_type(), &ctx));
}

#[test]
fn member_is_subtype_of_a_union_containing_it() {
    let base = harness::load_test_base();
    let dummy = pkl_analysis::parse("").unwrap();
    let ctx = ctx(&base, &dummy);

    let int_or_string = Type::Union {
        left: Box::new(base.int_type().clone()),
        right: Box::new(base.string_type().clone()),
        constraints: vec![],
    };
    assert!(base.int_type().is_subtype_of(&int_or_string, &ctx));
    assert!(base.string_type().is_subtype_of(&int_or_string, &ctx));
    assert!(!base.boolean_type().is_subtype_of(&int_or_string, &ctx));
}

#[test]
fn type_is_subtype_of_itself() {
    let base = harness::load_test_base();
    let dummy = pkl_analysis::parse("").unwrap();
    let ctx = ctx(&base, &dummy);

    assert!(base.string_type().is_subtype_of(base.string_type(), &ctx));
    assert!(base.int_type().is_subtype_of(base.int_type(), &ctx));
    assert!(Type::Unknown.is_subtype_of(&Type::Unknown, &ctx));
    assert!(Type::Nothing.is_subtype_of(&Type::Nothing, &ctx));
}
