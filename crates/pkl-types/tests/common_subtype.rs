//! `Type::has_common_subtype_with` tests.

mod harness;

use pkl_types::{AnalysisContext, FileId, Type};

fn ctx<'a>(
    base: &'a pkl_types::BaseModule,
    trees: &'a std::collections::HashMap<FileId, std::sync::Arc<pkl_syntax::ast::SyntaxTree>>,
) -> AnalysisContext<'a> {
    AnalysisContext::new(base, trees, harness::empty_imports())
}

#[test]
fn unknown_has_no_common_subtype_with_anything() {
    let base = harness::load_test_base();
    let trees = harness::pinned(&base, std::sync::Arc::new(pkl_syntax::parse("").unwrap()));
    let ctx = ctx(&base, &trees);

    assert!(!Type::Unknown.has_common_subtype_with(base.string_type(), &ctx));
    assert!(!Type::Unknown.has_common_subtype_with(base.int_type(), &ctx));
}

#[test]
fn anything_has_no_common_subtype_with_unknown() {
    let base = harness::load_test_base();
    let trees = harness::pinned(&base, std::sync::Arc::new(pkl_syntax::parse("").unwrap()));
    let ctx = ctx(&base, &trees);

    assert!(
        !base
            .string_type()
            .has_common_subtype_with(&Type::Unknown, &ctx)
    );
    assert!(
        !base
            .int_type()
            .has_common_subtype_with(&Type::Unknown, &ctx)
    );
}

#[test]
fn nothing_has_no_common_subtype_with_anything() {
    let base = harness::load_test_base();
    let trees = harness::pinned(&base, std::sync::Arc::new(pkl_syntax::parse("").unwrap()));
    let ctx = ctx(&base, &trees);

    assert!(!Type::Nothing.has_common_subtype_with(base.int_type(), &ctx));
    assert!(
        !base
            .int_type()
            .has_common_subtype_with(&Type::Nothing, &ctx)
    );
}

#[test]
fn int_and_string_are_disjoint() {
    let base = harness::load_test_base();
    let trees = harness::pinned(&base, std::sync::Arc::new(pkl_syntax::parse("").unwrap()));
    let ctx = ctx(&base, &trees);

    assert!(
        !base
            .int_type()
            .has_common_subtype_with(base.string_type(), &ctx)
    );
    assert!(
        !base
            .string_type()
            .has_common_subtype_with(base.int_type(), &ctx)
    );
}

#[test]
fn int_and_boolean_are_disjoint() {
    let base = harness::load_test_base();
    let trees = harness::pinned(&base, std::sync::Arc::new(pkl_syntax::parse("").unwrap()));
    let ctx = ctx(&base, &trees);

    assert!(
        !base
            .int_type()
            .has_common_subtype_with(base.boolean_type(), &ctx)
    );
}

#[test]
fn int_and_float_are_disjoint() {
    // Int and Float are concrete sibling classes under Number — neither is a
    // subclass of the other and no class in base.pkl extends both.
    let base = harness::load_test_base();
    let trees = harness::pinned(&base, std::sync::Arc::new(pkl_syntax::parse("").unwrap()));
    let ctx = ctx(&base, &trees);

    assert!(
        !base
            .int_type()
            .has_common_subtype_with(base.float_type(), &ctx)
    );
}

#[test]
fn int_and_number_overlap() {
    let base = harness::load_test_base();
    let trees = harness::pinned(&base, std::sync::Arc::new(pkl_syntax::parse("").unwrap()));
    let ctx = ctx(&base, &trees);

    assert!(
        base.int_type()
            .has_common_subtype_with(base.number_type(), &ctx)
    );
    assert!(
        base.number_type()
            .has_common_subtype_with(base.int_type(), &ctx)
    );
}

#[test]
fn union_overlaps_if_one_branch_does() {
    let base = harness::load_test_base();
    let trees = harness::pinned(&base, std::sync::Arc::new(pkl_syntax::parse("").unwrap()));
    let ctx = ctx(&base, &trees);

    let int_or_string = Type::union([base.int_type().clone(), base.string_type().clone()], &ctx);

    // String branch overlaps with String.
    assert!(int_or_string.has_common_subtype_with(base.string_type(), &ctx));
    // ...and symmetrically.
    assert!(
        base.string_type()
            .has_common_subtype_with(&int_or_string, &ctx)
    );
}

#[test]
fn union_is_disjoint_when_no_branch_overlaps() {
    let base = harness::load_test_base();
    let trees = harness::pinned(&base, std::sync::Arc::new(pkl_syntax::parse("").unwrap()));
    let ctx = ctx(&base, &trees);

    let int_or_string = Type::union([base.int_type().clone(), base.string_type().clone()], &ctx);

    // Boolean overlaps with neither Int nor String.
    assert!(!int_or_string.has_common_subtype_with(base.boolean_type(), &ctx));
    assert!(
        !base
            .boolean_type()
            .has_common_subtype_with(&int_or_string, &ctx)
    );
}

#[test]
fn union_of_unions_propagates_correctly() {
    let base = harness::load_test_base();
    let trees = harness::pinned(&base, std::sync::Arc::new(pkl_syntax::parse("").unwrap()));
    let ctx = ctx(&base, &trees);

    // (Int | String) | Boolean  vs  Boolean  → true, via the right branch.
    let inner = Type::union([base.int_type().clone(), base.string_type().clone()], &ctx);
    let outer = Type::union([inner, base.boolean_type().clone()], &ctx);

    assert!(outer.has_common_subtype_with(base.boolean_type(), &ctx));
}

#[test]
fn distinct_string_literals_are_disjoint() {
    let base = harness::load_test_base();
    let trees = harness::pinned(&base, std::sync::Arc::new(pkl_syntax::parse("").unwrap()));
    let ctx = ctx(&base, &trees);

    let a = Type::string_literal("a");
    let b = Type::string_literal("b");

    assert!(!a.has_common_subtype_with(&b, &ctx));
}

#[test]
fn string_literal_is_disjoint_from_int() {
    let base = harness::load_test_base();
    let trees = harness::pinned(&base, std::sync::Arc::new(pkl_syntax::parse("").unwrap()));
    let ctx = ctx(&base, &trees);

    let hello = Type::string_literal("hello");
    assert!(!hello.has_common_subtype_with(base.int_type(), &ctx));
    assert!(!base.int_type().has_common_subtype_with(&hello, &ctx));
}

#[test]
fn symmetric_on_disjoint_pairs() {
    let base = harness::load_test_base();
    let trees = harness::pinned(&base, std::sync::Arc::new(pkl_syntax::parse("").unwrap()));
    let ctx = ctx(&base, &trees);

    let pairs: Vec<(&Type, &Type)> = vec![
        (base.int_type(), base.string_type()),
        (base.int_type(), base.boolean_type()),
        (base.string_type(), base.boolean_type()),
    ];

    for (a, b) in pairs {
        assert_eq!(
            a.has_common_subtype_with(b, &ctx),
            b.has_common_subtype_with(a, &ctx),
            "asymmetric result for {:?} vs {:?}",
            a.render(&ctx),
            b.render(&ctx),
        );
    }
}

#[test]
fn non_null_alias_overlaps_with_int() {
    let base = harness::load_test_base();
    let trees = harness::pinned(&base, std::sync::Arc::new(pkl_syntax::parse("").unwrap()));
    let ctx = ctx(&base, &trees);

    assert!(
        base.non_null_type()
            .has_common_subtype_with(base.int_type(), &ctx)
    );
}
