mod harness;

use lspkl_types::find_member;

#[test]
fn parameterized_container_defaults_preserve_all_bindings() {
    let base = harness::load_test_base();
    let db = base.db.borrow();
    for (container, args, expected) in [
        (
            base.mapping_type(),
            vec![base.string_type().clone(), base.int_type().clone()],
            "Function1<String, Int>",
        ),
        (
            base.mapping_type(),
            vec![
                base.int_type().clone(),
                base.list_type()
                    .clone()
                    .with_type_args(vec![base.string_type().clone()]),
            ],
            "Function1<Int, List<String>>",
        ),
        (
            base.listing_type(),
            vec![base.string_type().clone()],
            "Function1<Int, String>",
        ),
    ] {
        let ty = container.clone().with_type_args(args);
        let resolved = find_member(&*db, &ty, "default").unwrap();
        assert_eq!(resolved.node.file, base.file_id());
        assert_eq!(resolved.ty.render(&*db), expected);
    }
}
