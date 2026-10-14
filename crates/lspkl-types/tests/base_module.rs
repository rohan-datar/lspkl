//! Tests for `BaseModule::from_tree` and well-known type lookup.
//!
//! These pin the contract: after `from_tree`, every cached type accessor returns a
//! `Type::Class` whose class identity lives in `base.file_id()`.

mod harness;

use lspkl_types::Type;

#[test]
fn string_type_points_into_base_module() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    match base.string_type() {
        Type::Class { class_id, .. } => {
            assert_eq!(
                class_id.module(&*db).file(&*db),
                base.file_id(),
                "well-known types must reference base.file_id()"
            );
        }
        other => panic!("string_type must be Type::Class, got: {other:?}"),
    }
}

#[test]
fn well_known_types_are_distinct() {
    // If two accessors return the same ClassId, the lookup logic is wrong
    // (probably grabbed the first class instead of the named one).
    let base = harness::load_test_base();
    let pairs: &[(&str, &Type, &Type)] = &[
        ("string vs int", base.string_type(), base.int_type()),
        ("int vs float", base.int_type(), base.float_type()),
        ("any vs null", base.any_type(), base.null_type()),
        (
            "listing vs mapping",
            base.listing_type(),
            base.mapping_type(),
        ),
    ];
    for (label, a, b) in pairs {
        assert_ne!(a, b, "{label}: accessors must return distinct types");
    }
}

#[test]
fn int_extends_number() {
    // Sanity-check that subtyping over the base hierarchy works at all.
    // Requires Type::is_subtype_of to be implemented.
    let base = harness::load_test_base();
    let db = base.db.borrow();
    let ctx = lspkl_types::AnalysisContext::for_root(&*db, base.file_id());

    assert!(
        base.int_type().is_subtype_of(base.number_type(), ctx.db),
        "Int must be a subtype of Number"
    );
    assert!(
        !base.number_type().is_subtype_of(base.int_type(), ctx.db),
        "Number must NOT be a subtype of Int"
    );
}
