//! Tests for `BaseModule::load` and well-known type lookup.
//!
//! These pin the contract: after `load`, every cached type accessor returns a
//! `Type::Class` whose `class_node` lives in `FileId::BASE`.

mod harness;

use pkl_types::{FileId, Type};

#[test]
fn loads_without_panicking() {
    let _base = harness::load_test_base();
}

#[test]
fn file_id_is_base() {
    let base = harness::load_test_base();
    assert_eq!(base.file_id(), FileId::BASE);
}

#[test]
fn string_type_points_into_base_module() {
    let base = harness::load_test_base();
    match base.string_type() {
        Type::Class { class_node, .. } => {
            assert_eq!(
                class_node.file_id,
                FileId::BASE,
                "well-known types must reference FileId::BASE"
            );
        }
        other => panic!("string_type must be Type::Class, got: {other:?}"),
    }
}

#[test]
fn well_known_types_are_distinct() {
    // If two accessors return the same NodeRef, the lookup logic is wrong
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
    let dummy_tree = pkl_syntax::parse("").expect("empty parse");
    let ctx = pkl_types::AnalysisContext::new(&dummy_tree, FileId(1), &base);

    assert!(
        base.int_type().is_subtype_of(base.number_type(), &ctx),
        "Int must be a subtype of Number"
    );
    assert!(
        !base.number_type().is_subtype_of(base.int_type(), &ctx),
        "Number must NOT be a subtype of Int"
    );
}
