mod harness;

use lspkl_syntax::ast::nodes;
use lspkl_types::{AnalysisContext, NodeRef, Type};
use std::time::{Duration, Instant};

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
fn module_class_arguments_follow_instance_subtyping_and_render_nonempty_names() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        r#"
import "pkl:math"
class Base {}
actual = getClass()
mathActual = math.getClass()
c1: Class<module>
c2: Class<math>
c3: Class<this>
c4: Class<Object>
c5: Class<Any>
bad: Class<Base>
wrongModule: Class<math>
constrained: Class<this(true)>
parametric: Class<List<Int>>
"#,
        |ctx| {
            let actual = property_type(ctx, "actual");
            assert_eq!(actual.render(ctx.db), "Class<1>");
            for name in ["c1", "c3", "c4", "c5"] {
                assert!(
                    actual.is_subtype_of(&property_type(ctx, name), ctx.db),
                    "{name}"
                );
            }
            assert!(
                property_type(ctx, "mathActual").is_subtype_of(&property_type(ctx, "c2"), ctx.db)
            );
            for name in ["bad", "wrongModule", "constrained", "parametric"] {
                assert!(
                    !actual.is_subtype_of(&property_type(ctx, name), ctx.db),
                    "{name}"
                );
            }
        },
    );
}

#[test]
fn syntax_node_type_alias_preserves_literal_choices() {
    let base = harness::load_test_base();
    let start = Instant::now();
    harness::with_snippet(
        &base,
        "import \"pkl:syntax\"\nx: syntax.NodeType = \"module\"",
        |ctx| {
            let ty = property_type(ctx, "x").unaliased(ctx.db);
            assert!(Type::string_literal("module").is_subtype_of(&ty, ctx.db));
            assert!(!Type::string_literal("not_a_node").is_subtype_of(&ty, ctx.db));
            let Type::Union { members } = ty else {
                panic!("expected syntax.NodeType choices")
            };
            assert!(members.len() > 100, "{} node choices", members.len());
        },
    );
    eprintln!("syntax.NodeType resolution: {:?}", start.elapsed());
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn literal_union_resolution_and_alias_expansion_scale_to_sdk_node_type() {
    let base = harness::load_test_base();
    for count in [40, 80, 200] {
        let alternatives = (0..count)
            .map(|i| format!("\"lit_{i}\""))
            .collect::<Vec<_>>()
            .join(" | ");
        let source = format!("typealias T = {alternatives}\nx: T\ny: ({alternatives})");
        let start = Instant::now();
        harness::with_snippet(&base, &source, |ctx| {
            let alias = property_type(ctx, "x").unaliased(ctx.db);
            let direct = property_type(ctx, "y");
            assert_eq!(alias, direct);
            let Type::Union { members } = alias else {
                panic!("expected literal choices")
            };
            assert_eq!(members.len(), count);
        });
        let elapsed = start.elapsed();
        eprintln!("{count} literal alternatives (direct + alias): {elapsed:?}");
        // A loose guard against the former multi-second/cubic normalization,
        // not a sub-second wall-clock requirement on shared CI machines.
        assert!(
            elapsed < Duration::from_secs(5),
            "{count} literals: {elapsed:?}"
        );
    }
}
