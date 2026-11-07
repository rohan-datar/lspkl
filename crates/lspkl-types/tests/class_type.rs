mod harness;

use lspkl_types::{DeclId, ItemId, Type, decl_type, module_items};

fn property(ctx: &lspkl_types::AnalysisContext, name: &str) -> Type {
    let (index, _) = module_items(ctx.db, ctx.root)
        .properties
        .iter()
        .find(|(_, p)| p.declaration.name.text(ctx.db) == name)
        .unwrap();
    decl_type(
        ctx.db,
        DeclId {
            file: ctx.root,
            root: ctx.root,
            item: ItemId::Property(index),
        },
    )
}

#[test]
fn class_and_alias_values_have_metadata_types_without_changing_annotations() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "class Foo {}\ntypealias Alias = Foo\na = Foo\nb = Alias\nc: Foo = new Foo {}\n",
        |ctx| {
            assert_eq!(property(ctx, "a").render(ctx.db), "Class<Foo>");
            assert!(property(ctx, "a").class_equals(base.class_type()));
            assert_eq!(property(ctx, "b"), *base.type_alias_type());
            assert_eq!(property(ctx, "b").render(ctx.db), "TypeAlias");
            assert_eq!(property(ctx, "c").render(ctx.db), "Foo");
        },
    );
}

#[test]
fn class_arguments_are_covariant_and_raw_class_accepts_any_class() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "open class Base {}\nclass Sub extends Base {}\na = Sub\nb = Base\nraw: Class = Sub\nwrong: Class<String> = Int\n",
        |ctx| {
            let sub = property(ctx, "a");
            let parent = property(ctx, "b");
            assert!(sub.is_subtype_of(&parent, ctx.db));
            assert!(!parent.is_subtype_of(&sub, ctx.db));
            assert!(sub.is_subtype_of(&property(ctx, "raw"), ctx.db));
            assert!(!sub.is_subtype_of(&property(ctx, "wrong"), ctx.db));
            assert!(sub.has_common_subtype_with(&parent, ctx.db));
        },
    );
}

#[test]
fn class_rejects_non_class_arguments_but_unknown_remains_permissive() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "a = Int\nun: Class<Int|String> = Int\nnullable: Class<Int?> = Int\nliteral: Class<\"x\"> = Int\nparametric: Class<List<Int>> = List\nbottom: Class<nothing> = Int\nalias: Class<NumberAlias> = Int\ntypealias NumberAlias = Number\n",
        |ctx| {
            let actual = property(ctx, "a");
            for name in ["un", "nullable", "literal", "parametric", "bottom"] {
                let expected = property(ctx, name);
                assert!(!actual.is_subtype_of(&expected, ctx.db), "{name}");
                assert!(!actual.has_common_subtype_with(&expected, ctx.db), "{name}");
            }
            assert!(actual.is_subtype_of(&property(ctx, "alias"), ctx.db));
            assert!(
                actual
                    .clone()
                    .with_type_args(vec![Type::Unknown])
                    .is_subtype_of(&property(ctx, "bottom"), ctx.db)
            );
            assert!(
                actual.is_subtype_of(&actual.clone().with_type_args(vec![Type::Unknown]), ctx.db)
            );
            assert!(
                actual
                    .clone()
                    .with_type_args(vec![Type::Unknown])
                    .is_subtype_of(&actual, ctx.db)
            );
        },
    );
}

#[test]
fn qualified_class_and_alias_values_use_sdk_metadata_types() {
    let base = harness::load_test_base();
    base.add(2, "class Foo {}\ntypealias Alias = Foo\n");
    harness::with_snippet(
        &base,
        "import \"2.pkl\" as lib\nclass Class {}\nclass TypeAlias {}\na = lib.Foo\nb = lib.Alias\n",
        |ctx| {
            assert_eq!(property(ctx, "a").render(ctx.db), "Class<Foo>");
            assert_eq!(property(ctx, "b").render(ctx.db), "TypeAlias");
            let Type::Class { class_id, .. } = property(ctx, "a") else {
                panic!("metadata class")
            };
            assert_eq!(class_id.module(ctx.db).file(ctx.db), base.file_id());
            let Type::Class { class_id, .. } = property(ctx, "b") else {
                panic!("metadata class")
            };
            assert_eq!(class_id.module(ctx.db).file(ctx.db), base.file_id());
        },
    );
}
