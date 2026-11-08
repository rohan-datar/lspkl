mod harness;

use lspkl_syntax::ast::nodes;
use lspkl_types::{AnalysisContext, ConstraintExpr, NodeRef, Type};

fn property_type(ctx: &AnalysisContext, name: &str) -> Type {
    let file = harness::file(ctx, 1);
    let tree = ctx.tree(file).unwrap();
    let property = nodes::Module::cast(tree, tree.root())
        .unwrap()
        .members(tree)
        .find(|&node| tree.name(node) == Some(name))
        .unwrap();
    lspkl_types::resolver::definition_type(ctx.db, NodeRef::new(file, property), ctx.root)
}

#[test]
fn stricter_intervals_and_alias_chains_are_subtypes() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        r#"
        typealias Positive = Int(this > 0)
        typealias Strong = Positive(this > 5)
        typealias Chain = Strong
        strong: Chain
        weak: Positive
        plain: Int
        bounded: Int(isBetween(6, 10))
        opaque: Int(custom())
        opaque2: Int(custom())
        other: Int(other())
    "#,
        |ctx| {
            let ty = |name| property_type(ctx, name);
            assert!(ty("strong").is_subtype_of(&ty("weak"), ctx.db));
            assert!(ty("bounded").is_subtype_of(&ty("weak"), ctx.db));
            assert!(!ty("weak").is_subtype_of(&ty("strong"), ctx.db));
            assert!(!ty("plain").is_subtype_of(&ty("weak"), ctx.db));
            assert!(ty("opaque").is_subtype_of(&ty("opaque2"), ctx.db));
            assert!(!ty("opaque").is_subtype_of(&ty("other"), ctx.db));
            assert_eq!(
                ty("strong").unaliased(ctx.db).render(ctx.db),
                "Int(this > 0, this > 5)"
            );
        },
    );
}

#[test]
fn constrained_union_compatibility_preserves_every_refinement() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        r#"
        strong: Int(this > 5)|String(length > 5)
        weak: Int(this > 0)|String(length > 0)
        mixed: Int(this > 5)|String
        outerUnion: (Int|Float)(this > 5)
        number: Number(this > 0)
    "#,
        |ctx| {
            let ty = |name| property_type(ctx, name);
            assert!(ty("strong").is_subtype_of(&ty("weak"), ctx.db));
            assert!(!ty("mixed").is_subtype_of(&ty("weak"), ctx.db));
            assert!(ty("outerUnion").is_subtype_of(&ty("number"), ctx.db));
            assert!(!ty("weak").is_subtype_of(&ty("strong"), ctx.db));
            let string = Type::string_literal("hello");
            assert!(string.is_subtype_of(
                &Type::constrained(
                    base.string_type().clone(),
                    vec![ConstraintExpr::opaque("length > 0")]
                ),
                ctx.db
            ));
        },
    );
}

#[test]
fn undecidable_refinements_preserve_possible_nullability() {
    let base = harness::load_test_base();
    harness::with_ctx(&base, |ctx| {
        let nullable = Type::constrained(
            base.int_type().clone().nullable(ctx.db),
            vec![ConstraintExpr::opaque("custom()")],
        );
        assert!(nullable.is_nullable(ctx.db));
        assert!(!nullable.non_null(ctx.db).is_nullable(ctx.db));
    });
}
