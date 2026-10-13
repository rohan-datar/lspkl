mod harness;

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use pkl_syntax::ast::{NodeKind, nodes};
use pkl_types::{AnalysisContext, ConstraintExpr, NodeRef, Type};

fn hash(ty: &Type) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    ty.hash(&mut hasher);
    hasher.finish()
}

fn property_type(ctx: &AnalysisContext, name: &str) -> Type {
    let tree = ctx.tree(harness::file(ctx, 1)).unwrap();
    let property = nodes::Module::cast(tree, tree.root())
        .unwrap()
        .members(tree)
        .find(|&node| tree.name(node) == Some(name))
        .unwrap();
    pkl_types::resolver::definition_type(
        ctx.db,
        NodeRef::new(harness::file(ctx, 1), property),
        ctx.root,
    )
}

#[test]
fn equivalent_unions_have_canonical_equality_hash_and_rendering() {
    let base = harness::load_test_base();
    harness::with_ctx(&base, |ctx| {
        let a = base.int_type().clone();
        let b = base.string_type().clone();
        let c = base.boolean_type().clone();
        let expected = Type::union([a.clone(), b.clone(), c.clone()], ctx.db);
        let nested = Type::union(
            [
                c.clone(),
                Type::union([b.clone(), a.clone()], ctx.db),
                b.clone(),
            ],
            ctx.db,
        );
        let reversed = Type::union([c, b, a], ctx.db);
        assert_eq!(expected, nested);
        assert_eq!(expected, reversed);
        assert_eq!(hash(&expected), hash(&nested));
        assert_eq!(hash(&expected), hash(&reversed));
        assert_eq!(expected.render(ctx.db), nested.render(ctx.db));
        assert_eq!(HashSet::from([expected, nested, reversed]).len(), 1);
    });
}

#[test]
fn syntax_normalizes_duplicate_and_nested_nullable_types() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "class A {}\nclass B {}\nx: (A?|B)?\ny: A|A\nz: A\nw: (A|B)?",
        |ctx| {
            assert_eq!(property_type(ctx, "x"), property_type(ctx, "w"));
            assert_eq!(property_type(ctx, "y"), property_type(ctx, "z"));
            assert_eq!(property_type(ctx, "x").render(ctx.db), "(A|B)?");
            let ty = property_type(ctx, "x");
            assert_eq!(ty.clone().nullable(ctx.db), ty);
        },
    );
}

#[test]
fn union_bottom_unknown_and_subtype_absorption_rules() {
    let base = harness::load_test_base();
    harness::with_ctx(&base, |ctx| {
        let int = base.int_type().clone();
        let number = base.number_type().clone();
        assert_eq!(Type::union([], ctx.db), Type::Nothing);
        assert_eq!(
            Type::union([Type::Nothing, Type::Nothing], ctx.db),
            Type::Nothing
        );
        assert_eq!(Type::union([Type::Nothing, int.clone()], ctx.db), int);
        assert_eq!(Type::union([int.clone(), int.clone()], ctx.db), int);
        assert_eq!(Type::union([int.clone(), number.clone()], ctx.db), number);
        for types in [
            vec![Type::Unknown, int.clone(), Type::Nothing],
            vec![Type::Nothing, int, Type::Unknown],
        ] {
            assert_eq!(Type::union(types, ctx.db), Type::Unknown);
        }
    });
}

#[test]
fn union_preserves_constraints_and_string_literal_choices() {
    let base = harness::load_test_base();
    harness::with_ctx(&base, |ctx| {
        let positive = Type::constrained(
            base.int_type().clone(),
            vec![ConstraintExpr::opaque("this > 0")],
        );
        let negative = Type::constrained(
            base.int_type().clone(),
            vec![ConstraintExpr::opaque("this < 0")],
        );
        let union = Type::union([positive.clone(), negative.clone()], ctx.db);
        let Type::Union { members } = &union else {
            panic!("constraint distinctions were absorbed")
        };
        assert_eq!(members.len(), 2);
        assert!(members.contains(&positive));
        assert!(members.contains(&negative));
        let choices = Type::union(
            [
                base.string_type().clone(),
                Type::string_literal("red"),
                Type::string_literal("blue"),
            ],
            ctx.db,
        );
        let Type::Union { members } = &choices else {
            panic!("literal choices were absorbed")
        };
        assert_eq!(members.len(), 3);
        assert!(members.contains(&Type::string_literal("red")));
        assert!(members.contains(&Type::string_literal("blue")));
    });
}

#[test]
fn constraints_merge_and_preserve_generic_container_and_member_queries() {
    let base = harness::load_test_base();
    harness::with_ctx(&base, |ctx| {
        let first = ConstraintExpr::opaque("length > 0");
        let second = ConstraintExpr::opaque("length < 10");
        let list = base
            .list_type()
            .clone()
            .with_type_args(vec![base.int_type().clone()]);
        let constrained = Type::constrained(
            Type::constrained(list.clone(), vec![first.clone()]),
            vec![second.clone()],
        );
        assert_eq!(constrained.constraints(), &[first, second]);
        assert_eq!(constrained.unconstrained(), &list);
        assert_eq!(
            pkl_types::resolver::container_value_type(&constrained, ctx),
            Some(base.int_type().clone())
        );
        assert_eq!(
            pkl_types::resolver::container_key_type(&constrained, ctx),
            Some(base.int_type().clone())
        );
        assert_eq!(
            pkl_types::find_member(ctx.db, &constrained, "length")
                .unwrap()
                .ty,
            *base.int_type()
        );
        let changed = constrained.with_type_args(vec![base.string_type().clone()]);
        assert_eq!(changed.constraints().len(), 2);
        assert_eq!(
            pkl_types::resolver::container_value_type(&changed, ctx),
            Some(base.string_type().clone())
        );
        assert_eq!(Type::constrained(list.clone(), vec![]), list);
        assert_eq!(
            Type::constrained(Type::Unknown, vec![ConstraintExpr::opaque("false")]),
            Type::Unknown
        );
        assert_eq!(
            Type::constrained(Type::Nothing, vec![ConstraintExpr::opaque("false")]),
            Type::Nothing
        );
    });
}

#[test]
fn generic_aliases_keep_identity_arguments_and_constraints() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "typealias Entries<T> = List<T>\nx: Entries<String>(length > 0)",
        |ctx| {
            let alias = property_type(ctx, "x");
            assert_eq!(alias.constraints().len(), 1);
            let Type::Alias { type_args, .. } = alias.unconstrained() else {
                panic!("alias identity lost")
            };
            assert_eq!(type_args, &[base.string_type().clone()]);
            assert!(alias.render(ctx.db).starts_with("Entries<String>"));
            assert_eq!(
                pkl_types::resolver::container_value_type(&alias, ctx),
                Some(base.string_type().clone())
            );
            assert_eq!(
                pkl_types::find_member(ctx.db, &alias, "length").unwrap().ty,
                *base.int_type()
            );
            let resolved = alias.unaliased(ctx.db);
            assert_eq!(resolved.constraints().len(), 1);
            assert_eq!(
                resolved.unconstrained(),
                &base
                    .list_type()
                    .clone()
                    .with_type_args(vec![base.string_type().clone()])
            );
            let union = Type::union([alias.clone(), alias.clone()], ctx.db);
            assert_eq!(union, alias);
        },
    );
}

#[test]
fn member_sets_intersect_all_branches_but_lookup_keeps_available_members() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "class A { shared: Int\npartial: Int }\nclass B { shared: String\npartial: String }\nclass C { shared: Boolean }",
        |ctx| {
            let tree = ctx.tree(harness::file(ctx, 1)).unwrap();
            let types = nodes::Module::cast(tree, tree.root())
                .unwrap()
                .members(tree)
                .filter(|&node| matches!(tree.kind(node), NodeKind::Class))
                .map(|node| {
                    Type::class(
                        pkl_types::class_id(
                            ctx.db,
                            NodeRef::new(harness::file(ctx, 1), node),
                            ctx.root,
                        )
                        .expect("class declaration"),
                    )
                });
            let union = Type::union(types, ctx.db);
            let members = pkl_types::members_of_type(ctx.db, &union);
            assert_eq!(
                members
                    .iter()
                    .find(|member| member.name == "shared")
                    .unwrap()
                    .ty,
                Type::union(
                    [
                        base.int_type().clone(),
                        base.string_type().clone(),
                        base.boolean_type().clone()
                    ],
                    ctx.db
                )
            );
            assert!(!members.iter().any(|member| member.name == "partial"));
            assert_eq!(
                pkl_types::find_member(ctx.db, &union, "shared").unwrap().ty,
                Type::union(
                    [
                        base.int_type().clone(),
                        base.string_type().clone(),
                        base.boolean_type().clone()
                    ],
                    ctx.db
                )
            );
            assert_eq!(
                pkl_types::find_member(ctx.db, &union, "partial")
                    .unwrap()
                    .ty,
                Type::union(
                    [base.int_type().clone(), base.string_type().clone()],
                    ctx.db
                )
            );
        },
    );
}

#[test]
fn module_identity_and_rendering_ignore_import_paths_and_aliases() {
    let base = harness::load_test_base();
    let source = Arc::new(
        pkl_syntax::parse("import \"2.pkl\" as direct\nimport \"nested/../2.pkl\" as alternate")
            .unwrap(),
    );
    let target = Arc::new(pkl_syntax::parse("module example.Target\nvalue = 1").unwrap());
    base.add(1, source.source());
    base.add(2, target.source());
    let db = base.db.borrow();
    let ctx = AnalysisContext::new(
        &base,
        &*db,
        pkl_db::Db::file(&*db, &harness::uri(1)).unwrap(),
    );
    let position = NodeRef::new(harness::file(&ctx, 1), source.root());
    let direct = pkl_types::resolver::resolve_unqualified_access(position, "direct", &ctx)
        .unwrap()
        .ty;
    let alternate = pkl_types::resolver::resolve_unqualified_access(position, "alternate", &ctx)
        .unwrap()
        .ty;
    assert_eq!(direct, alternate);
    assert_eq!(hash(&direct), hash(&alternate));
    assert_eq!(direct.render(ctx.db), "Target");
    assert_eq!(Type::union([direct.clone(), alternate], ctx.db), direct);
}

#[test]
fn expression_and_container_unions_normalize_without_losing_generic_arguments() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "a: List<Int>\nb: List<String>\ncombined = a + b\nsame = if (true) 1 else 2\nbottom = if (true) 1 else throw(\"failure\")",
        |ctx| {
            assert_eq!(property_type(ctx, "same"), *base.int_type());
            assert_eq!(property_type(ctx, "bottom"), *base.int_type());
            let combined = property_type(ctx, "combined");
            assert_eq!(
                combined,
                base.list_type().clone().with_type_args(vec![Type::union(
                    [base.int_type().clone(), base.string_type().clone()],
                    ctx.db
                ),])
            );
            assert_eq!(
                pkl_types::resolver::container_value_type(&combined, ctx),
                Some(Type::union(
                    [base.int_type().clone(), base.string_type().clone()],
                    ctx.db
                ))
            );
        },
    );
}

#[test]
fn constrained_generic_class_member_substitutes_the_argument() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "class Box<T> { value: T }\nx: Box<String>(true)\ny: Box<Int>|Box<String>",
        |ctx| {
            let ty = property_type(ctx, "x");
            assert_eq!(ty.constraints().len(), 1);
            assert_eq!(
                pkl_types::find_member(ctx.db, &ty, "value").unwrap().ty,
                *base.string_type()
            );
            let members = pkl_types::members_of_type(ctx.db, &ty);
            assert_eq!(
                members
                    .iter()
                    .find(|member| member.name == "value")
                    .unwrap()
                    .ty,
                *base.string_type()
            );
            let alternatives = property_type(ctx, "y");
            let members = pkl_types::members_of_type(ctx.db, &alternatives);
            assert_eq!(
                members
                    .iter()
                    .find(|member| member.name == "value")
                    .unwrap()
                    .ty,
                Type::union(
                    [base.int_type().clone(), base.string_type().clone()],
                    ctx.db
                )
            );
        },
    );
}

#[test]
fn alias_union_absorption_preserves_refinements_arguments_and_literals() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        concat!(
            "typealias Choices = Int(this > 0)|String\n",
            "typealias Lists = List<Int>|List<String>\n",
            "typealias TextOrInt = String|Int\n",
            "refined: Choices|Int(this < 0)\n",
            "generic: Lists|List<Boolean>\n",
            "literal: TextOrInt|\"red\"\n",
        ),
        |ctx| {
            let refined = property_type(ctx, "refined");
            let Type::Union { members } = &refined else {
                panic!("refinement erased")
            };
            assert!(
                members
                    .iter()
                    .any(|ty| ty.constraints() == [ConstraintExpr::opaque("this < 0")])
            );
            let generic = property_type(ctx, "generic");
            let Type::Union { members } = &generic else {
                panic!("generic alternative erased")
            };
            assert!(
                members.contains(
                    &base
                        .list_type()
                        .clone()
                        .with_type_args(vec![base.boolean_type().clone()])
                )
            );
            let literal = property_type(ctx, "literal");
            let Type::Union { members } = &literal else {
                panic!("literal alternative erased")
            };
            assert!(members.contains(&Type::string_literal("red")));
        },
    );
}

#[test]
fn generic_alias_chains_preserve_distinct_instantiations() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "typealias Entries<T> = List<T>\ntypealias Forward<U> = Entries<U>\nx: Forward<String>\ny: Forward<Int>|Forward<String>",
        |ctx| {
            assert_eq!(
                pkl_types::resolver::container_value_type(&property_type(ctx, "x"), ctx),
                Some(base.string_type().clone())
            );
            let ty = property_type(ctx, "y");
            let Type::Union { members } = &ty else {
                panic!("distinct instantiation erased")
            };
            let values: HashSet<_> = members
                .iter()
                .map(|ty| pkl_types::resolver::container_value_type(ty, ctx).unwrap())
                .collect();
            assert_eq!(
                values,
                HashSet::from([base.int_type().clone(), base.string_type().clone()])
            );
        },
    );
}

#[test]
fn non_null_keeps_every_non_null_branch_and_refinement() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "class A {}\nclass B {}\ntypealias Maybe = (A?|B)?\nx: Maybe\ny: A|B\nz: ((A?|B)?)(true)",
        |ctx| {
            let expected = property_type(ctx, "y");
            assert_eq!(property_type(ctx, "x").non_null(ctx.db), expected);
            let refined = property_type(ctx, "z").non_null(ctx.db);
            assert_eq!(refined.unconstrained(), &expected);
            assert_eq!(refined.constraints(), &[ConstraintExpr::opaque("true")]);
            assert_eq!(base.null_type().non_null(ctx.db), Type::Nothing);
        },
    );
}

#[test]
fn recursive_aliases_recover_without_crashing() {
    const CHILD: &str = "LSPKL_TYPE_ALIAS_CYCLE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "recursive_aliases_recover_without_crashing",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "alias subprocess failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let base = harness::load_test_base();
    for source in [
        "typealias Loop = Loop|Int\nx: Loop|String\ny: Loop",
        "typealias Loop = Loop?\nx: Loop|String\ny: Loop",
        "typealias Loop = Other|Int\ntypealias Other = Loop|String\nx: Loop|String\ny: Loop",
        "typealias Loop<T> = Loop<List<T>>|Int\nx: Loop<String>|String\ny: Loop<String>",
    ] {
        harness::with_snippet(&base, source, |ctx| {
            let mixed = property_type(ctx, "x");
            assert_eq!(property_type(ctx, "y").unaliased(ctx.db), Type::Unknown);
            assert_eq!(property_type(ctx, "y").non_null(ctx.db), Type::Unknown);
            assert!(base.int_type().is_subtype_of(&mixed, ctx.db));
            assert_eq!(
                pkl_types::find_member(ctx.db, &mixed, "length").unwrap().ty,
                *base.int_type()
            );
        });
    }
}

#[test]
fn finite_nested_alias_arguments_are_not_recursive_bodies() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        concat!(
            "typealias Identity<T> = T\n",
            "typealias Forward<U> = Identity<U>\n",
            "repeat: Identity<Identity<String>>\n",
            "refined: Identity<Identity<Int(this > 0)>>\n",
            "nullable: Identity<Identity<Int?>>\n",
            "alternate: Forward<Identity<Forward<String>>>\n",
            "contained: Identity<List<Identity<Int(this > 0)>>>\n",
        ),
        |ctx| {
            assert_eq!(
                property_type(ctx, "repeat").unaliased(ctx.db),
                *base.string_type()
            );
            assert_eq!(
                property_type(ctx, "alternate").unaliased(ctx.db),
                *base.string_type()
            );
            let refined = property_type(ctx, "refined");
            assert!(refined.has_constraints(ctx.db));
            assert_eq!(refined.unaliased(ctx.db).unconstrained(), base.int_type());
            assert_eq!(
                refined.unaliased(ctx.db).constraints(),
                &[ConstraintExpr::opaque("this > 0")]
            );
            let contained = property_type(ctx, "contained");
            assert!(contained.has_constraints(ctx.db));
            let element = pkl_types::resolver::container_value_type(&contained, ctx).unwrap();
            assert_eq!(element.unconstrained(), base.int_type());
            assert_eq!(element.constraints(), &[ConstraintExpr::opaque("this > 0")]);
            let nullable = property_type(ctx, "nullable");
            assert_eq!(nullable.unaliased(ctx.db), base.int_type().nullable(ctx.db));
            assert_eq!(nullable.non_null(ctx.db), *base.int_type());
        },
    );
}
