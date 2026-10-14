mod harness;

use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};
use pkl_types::{AnalysisContext, NodeRef, Scope, ScopeKind, Type, scope_at};
use std::collections::HashSet;

fn find(tree: &SyntaxTree, kind: NodeKind, text: &str, occurrence: usize) -> NodeId {
    fn visit(tree: &SyntaxTree, id: NodeId, kind: &NodeKind, text: &str, found: &mut Vec<NodeId>) {
        if tree.kind(id) == kind && tree.text(id) == text {
            found.push(id);
        }
        for child in tree.children(id) {
            visit(tree, *child, kind, text, found);
        }
    }
    let mut found = Vec::new();
    visit(tree, tree.root(), &kind, text, &mut found);
    *found
        .get(occurrence)
        .unwrap_or_else(|| panic!("missing {kind:?} {text:?} occurrence {occurrence}"))
}

fn node(
    ctx: &AnalysisContext,
    file: u32,
    kind: NodeKind,
    text: &str,
    occurrence: usize,
) -> NodeRef {
    let file = harness::file(ctx, file);
    NodeRef::new(file, find(ctx.tree(file).unwrap(), kind, text, occurrence))
}

fn access_scope<'db>(ctx: &'db AnalysisContext<'db>, text: &str, occurrence: usize) -> Scope<'db> {
    let scope = scope_at(
        node(ctx, 1, NodeKind::UnqualifiedAccessExpr, text, occurrence),
        ctx,
    );
    assert_entries(&scope, ctx);
    scope
}

fn assert_entries(scope: &Scope, ctx: &AnalysisContext) {
    let mut names = HashSet::new();
    for offered in scope.entries() {
        let name = offered.name.text(ctx.db);
        assert!(names.insert(name.to_owned()), "duplicate completion {name}");
        let resolved = scope
            .lookup(name)
            .unwrap_or_else(|| panic!("offered name {name} is not resolvable"));
        assert_eq!(offered.kind, resolved.kind, "kind for {name}");
        let target = offered
            .node(ctx)
            .unwrap_or_else(|| panic!("missing target for {name}"));
        assert_eq!(
            Some(target),
            resolved.node(ctx),
            "file-qualified target for {name}"
        );
        assert!(ctx.tree(target.file).is_some(), "target file for {name}");
    }
}

fn assert_target(
    scope: &Scope,
    ctx: &AnalysisContext,
    name: &str,
    kind: ScopeKind,
    target: NodeRef,
    ty: Option<&Type>,
) {
    let entry = scope
        .lookup(name)
        .unwrap_or_else(|| panic!("missing {name}"));
    assert_eq!(entry.kind, kind, "kind for {name}");
    assert_eq!(entry.node(ctx), Some(target), "declaration for {name}");
    if let Some(ty) = ty {
        assert_eq!(&entry.ty(ctx), ty, "type for {name}");
    }
}
#[test]
fn constraint_parameters_precede_receiver_own_members() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        r#"function use(length: Int): String(length > 0) = """#,
        |ctx| {
            let scope = access_scope(ctx, "length", 0);
            assert_target(
                &scope,
                ctx,
                "length",
                ScopeKind::Local,
                node(ctx, 1, NodeKind::TypedIdentifier, "length: Int", 0),
                Some(base.int_type()),
            );
        },
    );
}

#[test]
fn nearest_object_members_shadow_outer_method_parameters() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "function build(value: Int) = new { value: String = \"object\"\n result = value }\n",
        |ctx| {
            let scope = access_scope(ctx, "value", 0);
            assert_target(
                &scope,
                ctx,
                "value",
                ScopeKind::Property,
                node(
                    ctx,
                    1,
                    NodeKind::ObjectProperty,
                    "value: String = \"object\"",
                    0,
                ),
                Some(base.string_type()),
            );
        },
    );
}

#[test]
fn inherited_constraint_members_follow_module_declarations_and_imports() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "getClass: Int = 1\ntypealias Label = String(getClass > 0)\n",
        |ctx| {
            let scope = access_scope(ctx, "getClass", 0);
            assert_target(
                &scope,
                ctx,
                "getClass",
                ScopeKind::Property,
                node(ctx, 1, NodeKind::ClassProperty, "getClass: Int = 1", 0),
                Some(base.int_type()),
            );
        },
    );
    base.add(2, "value: Int = 1\n");
    harness::with_snippet(
        &base,
        "import \"2.pkl\" as getClass\ntypealias Label = String(getClass != null)\n",
        |ctx| {
            let scope = access_scope(ctx, "getClass", 0);
            let file = harness::file(ctx, 2);
            let target = NodeRef::new(file, ctx.tree(file).unwrap().root());
            assert_target(
                &scope,
                ctx,
                "getClass",
                ScopeKind::Import,
                target,
                Some(&Type::Module {
                    module_id: pkl_types::module_id(ctx.db, target.file, ctx.root),
                }),
            );
        },
    );
}

#[test]
fn nested_locals_and_parameters_shadow_own_members() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        r#"
value: String = "module"
class Example {
  value: Boolean = true
  function probe(value: Int) = let (value: String = "local") value
  function parameter(value: Int) = value
  function member() = value
}
result = value
"#,
        |ctx| {
            let local = access_scope(ctx, "value", 0);
            assert_target(
                &local,
                ctx,
                "value",
                ScopeKind::Local,
                node(ctx, 1, NodeKind::TypedIdentifier, "value: String", 0),
                Some(base.string_type()),
            );
            let parameter = access_scope(ctx, "value", 1);
            assert_target(
                &parameter,
                ctx,
                "value",
                ScopeKind::Local,
                node(ctx, 1, NodeKind::TypedIdentifier, "value: Int", 1),
                Some(base.int_type()),
            );
            let member = access_scope(ctx, "value", 2);
            assert_target(
                &member,
                ctx,
                "value",
                ScopeKind::Property,
                node(ctx, 1, NodeKind::ClassProperty, "value: Boolean = true", 0),
                Some(base.boolean_type()),
            );
            let module = access_scope(ctx, "value", 3);
            assert_target(
                &module,
                ctx,
                "value",
                ScopeKind::Property,
                node(
                    ctx,
                    1,
                    NodeKind::ClassProperty,
                    "value: String = \"module\"",
                    0,
                ),
                Some(base.string_type()),
            );
        },
    );
}

#[test]
fn own_members_imports_and_inherited_members_have_lexical_precedence() {
    let base = harness::load_test_base();
    base.add(
        2,
        "open class Parent { inherited: Int = 1\ndep: Int = 2\nmoduleWins: Int = 3 }\n",
    );
    base.add(3, "remote: String = \"imported\"\n");
    harness::with_snippet(
        &base,
        r#"
import "2.pkl" as parent
import "3.pkl" as dep
moduleWins: String = "module"
class Child extends parent.Parent {
  own: Boolean = true
  function probe() = own
  function moduleProbe() = moduleWins
  function importProbe() = dep
  function inheritedProbe() = inherited
}
class OwnImport extends parent.Parent {
  dep: Boolean = false
  function probe() = dep
}
"#,
        |ctx| {
            let own = access_scope(ctx, "own", 0);
            assert_target(
                &own,
                ctx,
                "own",
                ScopeKind::Property,
                node(ctx, 1, NodeKind::ClassProperty, "own: Boolean = true", 0),
                Some(base.boolean_type()),
            );
            let module = access_scope(ctx, "moduleWins", 0);
            assert_target(
                &module,
                ctx,
                "moduleWins",
                ScopeKind::Property,
                node(
                    ctx,
                    1,
                    NodeKind::ClassProperty,
                    "moduleWins: String = \"module\"",
                    0,
                ),
                Some(base.string_type()),
            );
            let imported = access_scope(ctx, "dep", 0);
            let file = harness::file(ctx, 3);
            assert_target(
                &imported,
                ctx,
                "dep",
                ScopeKind::Import,
                NodeRef::new(file, ctx.tree(file).unwrap().root()),
                None,
            );
            assert_eq!(
                imported.lookup("dep").unwrap().ty(ctx),
                pkl_types::resolver::module_type(
                    NodeRef::new(file, ctx.tree(file).unwrap().root()),
                    ctx
                )
            );
            let inherited = access_scope(ctx, "inherited", 0);
            assert_target(
                &inherited,
                ctx,
                "inherited",
                ScopeKind::Property,
                node(ctx, 2, NodeKind::ClassProperty, "inherited: Int = 1", 0),
                Some(base.int_type()),
            );
            let own_import = access_scope(ctx, "dep", 1);
            assert_target(
                &own_import,
                ctx,
                "dep",
                ScopeKind::Property,
                node(ctx, 1, NodeKind::ClassProperty, "dep: Boolean = false", 0),
                Some(base.boolean_type()),
            );
            let int = inherited
                .lookup("Int")
                .expect("base namespace remains visible");
            assert_eq!(int.kind, ScopeKind::Class);
            assert_eq!(int.node(ctx).unwrap().file, base.file_id());
            assert_eq!(int.ty(ctx), base.int_type().clone());
        },
    );
}

#[test]
fn let_binding_is_hidden_in_initializer_and_visible_only_in_body() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "value: Int = 1\nresult = let (value: String = value) value\nafter = value\n",
        |ctx| {
            let outer = node(ctx, 1, NodeKind::ClassProperty, "value: Int = 1", 0);
            let initializer = access_scope(ctx, "value", 0);
            assert_target(
                &initializer,
                ctx,
                "value",
                ScopeKind::Property,
                outer,
                Some(base.int_type()),
            );
            let body = access_scope(ctx, "value", 1);
            assert_target(
                &body,
                ctx,
                "value",
                ScopeKind::Local,
                node(ctx, 1, NodeKind::TypedIdentifier, "value: String", 0),
                Some(base.string_type()),
            );
            let after = access_scope(ctx, "value", 2);
            assert_target(
                &after,
                ctx,
                "value",
                ScopeKind::Property,
                outer,
                Some(base.int_type()),
            );
        },
    );
}

#[test]
fn for_bindings_are_visible_in_body_but_not_iterable_or_siblings() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "items: List<String> = List(\"one\")\nresult = new { for (index, item in items) { key = index\nvalue = item }\nafter = items }\n",
        |ctx| {
            let iterable = access_scope(ctx, "items", 0);
            assert!(iterable.lookup("index").is_none());
            assert!(iterable.lookup("item").is_none());
            let body = access_scope(ctx, "item", 0);
            assert_target(
                &body,
                ctx,
                "item",
                ScopeKind::Local,
                node(ctx, 1, NodeKind::TypedIdentifier, "item", 0),
                Some(base.string_type()),
            );
            assert_target(
                &body,
                ctx,
                "index",
                ScopeKind::Local,
                node(ctx, 1, NodeKind::TypedIdentifier, "index", 0),
                Some(base.int_type()),
            );
            let key = access_scope(ctx, "index", 0);
            assert_target(
                &key,
                ctx,
                "item",
                ScopeKind::Local,
                node(ctx, 1, NodeKind::TypedIdentifier, "item", 0),
                Some(base.string_type()),
            );
            let sibling = access_scope(ctx, "items", 1);
            assert!(sibling.lookup("index").is_none());
            assert!(sibling.lookup("item").is_none());
        },
    );
}

#[test]
fn object_parameters_and_when_branch_members_do_not_leak() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "parameter: String = \"module\"\nresult = new { parameter: Int ->\n value = parameter\n when (true) { thenName: Int = 1\n thenResult = thenName } else { elseName: String = \"else\"\n elseResult = elseName }\n outside = parameter\n}\nafter = parameter\n",
        |ctx| {
            let parameter = node(ctx, 1, NodeKind::TypedIdentifier, "parameter: Int", 0);
            let body = access_scope(ctx, "parameter", 0);
            assert_target(
                &body,
                ctx,
                "parameter",
                ScopeKind::Local,
                parameter,
                Some(base.int_type()),
            );
            let then_scope = access_scope(ctx, "thenName", 0);
            assert_target(
                &then_scope,
                ctx,
                "thenName",
                ScopeKind::Property,
                node(ctx, 1, NodeKind::ObjectProperty, "thenName: Int = 1", 0),
                Some(base.int_type()),
            );
            assert_target(
                &then_scope,
                ctx,
                "parameter",
                ScopeKind::Local,
                parameter,
                Some(base.int_type()),
            );
            assert!(then_scope.lookup("elseName").is_none());
            let else_scope = access_scope(ctx, "elseName", 0);
            assert_target(
                &else_scope,
                ctx,
                "elseName",
                ScopeKind::Property,
                node(
                    ctx,
                    1,
                    NodeKind::ObjectProperty,
                    "elseName: String = \"else\"",
                    0,
                ),
                Some(base.string_type()),
            );
            assert!(else_scope.lookup("thenName").is_none());
            let outside = access_scope(ctx, "parameter", 1);
            assert!(outside.lookup("thenName").is_none());
            assert!(outside.lookup("elseName").is_none());
            let after = access_scope(ctx, "parameter", 2);
            assert_target(
                &after,
                ctx,
                "parameter",
                ScopeKind::Property,
                node(
                    ctx,
                    1,
                    NodeKind::ClassProperty,
                    "parameter: String = \"module\"",
                    0,
                ),
                Some(base.string_type()),
            );
        },
    );
}

#[test]
fn source_shift_refreshes_scope_targets_without_cross_file_confusion() {
    let base = harness::load_test_base();
    base.add(2, "value: Boolean = true\n");
    for source in [
        "value: Int = 1\nresult = value\n",
        "// shifted\npadding = true\nvalue: Int = 1\nresult = value\n",
    ] {
        base.add(1, source);
        let db = base.db.borrow();
        let ctx =
            AnalysisContext::for_root(&*db, pkl_db::Db::file(&*db, &harness::uri(1)).unwrap());
        let scope = access_scope(&ctx, "value", 0);
        assert_target(
            &scope,
            &ctx,
            "value",
            ScopeKind::Property,
            node(&ctx, 1, NodeKind::ClassProperty, "value: Int = 1", 0),
            Some(base.int_type()),
        );
        let other = scope_at(node(&ctx, 2, NodeKind::TrueLiteralExpr, "true", 0), &ctx);
        assert_entries(&other, &ctx);
        assert_target(
            &other,
            &ctx,
            "value",
            ScopeKind::Property,
            node(&ctx, 2, NodeKind::ClassProperty, "value: Boolean = true", 0),
            Some(base.boolean_type()),
        );
    }
}

#[test]
fn nested_let_and_function_parameters_preserve_outer_initializer_scope() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "result = (value: Int) -> let (value = true) let (value = value) value\n",
        |ctx| {
            let initializer = access_scope(ctx, "value", 0);
            assert_target(
                &initializer,
                ctx,
                "value",
                ScopeKind::Local,
                node(ctx, 1, NodeKind::TypedIdentifier, "value", 0),
                Some(base.boolean_type()),
            );
            let body = access_scope(ctx, "value", 1);
            assert_target(
                &body,
                ctx,
                "value",
                ScopeKind::Local,
                node(ctx, 1, NodeKind::TypedIdentifier, "value", 1),
                Some(base.boolean_type()),
            );
            let literal = node(ctx, 1, NodeKind::TrueLiteralExpr, "true", 0);
            let function = scope_at(literal, ctx);
            assert_entries(&function, ctx);
            assert_target(
                &function,
                ctx,
                "value",
                ScopeKind::Local,
                node(ctx, 1, NodeKind::TypedIdentifier, "value: Int", 0),
                Some(base.int_type()),
            );
        },
    );
}

#[test]
fn own_import_and_inherited_names_shadow_base_namespace() {
    let base = harness::load_test_base();
    base.add(2, "String: Int = 2\n");
    harness::with_snippet(&base, "String: Int = 1\nresult = String\n", |ctx| {
        let scope = access_scope(ctx, "String", 0);
        assert_target(
            &scope,
            ctx,
            "String",
            ScopeKind::Property,
            node(ctx, 1, NodeKind::ClassProperty, "String: Int = 1", 0),
            Some(base.int_type()),
        );
    });
    harness::with_snippet(
        &base,
        "import \"2.pkl\" as String\nresult = String\n",
        |ctx| {
            let scope = access_scope(ctx, "String", 0);
            let file = harness::file(ctx, 2);
            let root = NodeRef::new(file, ctx.tree(file).unwrap().root());
            assert_target(
                &scope,
                ctx,
                "String",
                ScopeKind::Import,
                root,
                Some(&pkl_types::resolver::module_type(root, ctx)),
            );
        },
    );
    harness::with_snippet(&base, "extends \"2.pkl\"\nresult = String\n", |ctx| {
        let scope = access_scope(ctx, "String", 0);
        assert_target(
            &scope,
            ctx,
            "String",
            ScopeKind::Property,
            node(ctx, 2, NodeKind::ClassProperty, "String: Int = 2", 0),
            Some(base.int_type()),
        );
    });
}
