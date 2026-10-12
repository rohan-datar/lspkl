mod harness;
use lsp_types::{CompletionResponse, GotoDefinitionResponse, HoverContents};
use pkl_syntax::ast::NodeKind;
use pkl_types::{AnalysisContext, FileId, ImportResolution, NodeRef};
use std::{collections::HashMap, sync::Arc};
fn with_graph(f: impl FnOnce(&AnalysisContext)) {
    let base = harness::load_test_base();
    let current=Arc::new(pkl_syntax::parse("extends \"parent.pkl\"\nimport \"lib.pkl\" as library\nimport* \"*.pkl\" as libs\nx = library.value\ny = inherited\nz = libs[\"lib.pkl\"].value\n").unwrap());
    let lib =
        Arc::new(pkl_syntax::parse("/// Imported docs\nvalue = 42\nclass SomeClass {}\n").unwrap());
    let parent = Arc::new(pkl_syntax::parse("inherited: String = \"yes\"\n").unwrap());
    let mut edges = HashMap::new();
    for (kind, target) in [
        (NodeKind::Import, NodeRef::new(FileId(2), lib.root())),
        (
            NodeKind::ModuleExtendsAmendsClause,
            NodeRef::new(FileId(3), parent.root()),
        ),
    ] {
        let node = find(&current, current.root(), &kind).unwrap();
        edges.insert(
            NodeRef::new(FileId(1), node),
            ImportResolution::Single(target),
        );
    }
    let glob = find(&current, current.root(), &NodeKind::ImportGlob).unwrap();
    edges.insert(
        NodeRef::new(FileId(1), glob),
        ImportResolution::Glob(vec![NodeRef::new(FileId(2), lib.root())]),
    );
    let trees = HashMap::from([
        (base.file_id(), base.tree().clone()),
        (FileId(1), current.clone()),
        (FileId(2), lib),
        (FileId(3), parent),
    ]);
    let ctx = AnalysisContext::new(&base, &trees, &edges);
    f(&ctx);
}
fn find(
    t: &pkl_syntax::ast::SyntaxTree,
    n: pkl_syntax::ast::NodeId,
    k: &NodeKind,
) -> Option<pkl_syntax::ast::NodeId> {
    if t.kind(n) == k {
        return Some(n);
    }
    t.children(n).iter().find_map(|&c| find(t, c, k))
}
fn labels(ctx: &AnalysisContext, line: u32, col: u32) -> Vec<String> {
    let offset = harness::offset(ctx.tree(pkl_types::FileId(1)).unwrap(), line, col);
    match lsp_features::completion::completion(
        ctx,
        pkl_types::InFile::new(pkl_types::FileId(1), offset),
        |range| harness::range(ctx.tree(pkl_types::FileId(1)).unwrap(), range),
    )
    .unwrap()
    {
        CompletionResponse::Array(v) => v.into_iter().map(|i| i.label).collect(),
        CompletionResponse::List(v) => v.items.into_iter().map(|i| i.label).collect(),
    }
}
#[test]
fn imported_hover_and_definition_use_target_tree() {
    with_graph(|ctx| {
        let h = lsp_features::hover::hover(
            ctx,
            pkl_types::InFile::new(
                pkl_types::FileId(1),
                harness::offset(ctx.tree(pkl_types::FileId(1)).unwrap(), 3, 13),
            ),
        )
        .unwrap();
        let HoverContents::Markup(m) = h.contents else {
            panic!("markdown")
        };
        assert!(m.value.contains("value: Int"));
        assert!(m.value.contains("Imported docs"));
        let d = lsp_features::goto_definition::goto_definition(
            ctx,
            pkl_types::InFile::new(
                pkl_types::FileId(1),
                harness::offset(ctx.tree(pkl_types::FileId(1)).unwrap(), 3, 13),
            ),
            |id| (id == FileId(2)).then(|| "file:///lib.pkl".parse().unwrap()),
            harness::range,
        )
        .unwrap();
        let GotoDefinitionResponse::Scalar(l) = d else {
            panic!("location")
        };
        let target = ctx.tree(FileId(2)).unwrap();
        let property = find(target, target.root(), &NodeKind::ClassProperty).unwrap();
        assert_eq!(l.range, harness::range(target, target.span(property)));
        assert_eq!(l.uri.as_str(), "file:///lib.pkl");
        assert!(
            lsp_features::hover::hover(
                ctx,
                pkl_types::InFile::new(
                    pkl_types::FileId(1),
                    harness::offset(ctx.tree(pkl_types::FileId(1)).unwrap(), 1, 21)
                )
            )
            .is_some()
        );
    });
}
#[test]
fn imported_inherited_and_glob_value_completions() {
    with_graph(|ctx| {
        let qualified = labels(ctx, 3, 13);
        assert!(qualified.contains(&"value".into()));
        assert!(qualified.contains(&"SomeClass".into()));
        let inherited = labels(ctx, 4, 9);
        assert!(inherited.contains(&"inherited".into()));
        assert!(inherited.contains(&"library".into()));
        assert!(inherited.contains(&"libs".into()));
        let glob = labels(ctx, 5, 25);
        assert!(glob.contains(&"value".into()), "{glob:?}");
    });
}

fn with_consumer_regressions(f: impl FnOnce(&AnalysisContext)) {
    let base = harness::load_test_base();
    let current=Arc::new(pkl_syntax::parse("extends \"parent.pkl\"\nimport \"lib.pkl\" as library\nimport \"lib.pkl\" as output\nx: library.Alias\nq = x.field\nz = output.value\ny = let (library = library.value) library\nw = library.f(foobar)\n").unwrap());
    let lib=Arc::new(pkl_syntax::parse("class C { field: Int }\ntypealias Alias = C\nvalue = 42\nfunction f(arg: String): String = arg\n").unwrap());
    let parent = Arc::new(pkl_syntax::parse("output: String = \"parent\"\n").unwrap());
    let mut edges = HashMap::new();
    for id in current.children_matching(current.root(), |k| matches!(k, NodeKind::Import)) {
        edges.insert(
            NodeRef::new(FileId(1), id),
            ImportResolution::Single(NodeRef::new(FileId(2), lib.root())),
        );
    }
    let clause = find(
        &current,
        current.root(),
        &NodeKind::ModuleExtendsAmendsClause,
    )
    .unwrap();
    edges.insert(
        NodeRef::new(FileId(1), clause),
        ImportResolution::Single(NodeRef::new(FileId(3), parent.root())),
    );
    let trees = HashMap::from([
        (base.file_id(), base.tree().clone()),
        (FileId(1), current.clone()),
        (FileId(2), lib),
        (FileId(3), parent),
    ]);
    f(&AnalysisContext::new(&base, &trees, &edges));
}
#[test]
fn lexical_import_precedes_inherited_output_in_resolution_and_completion() {
    with_consumer_regressions(|ctx| {
        let output = pkl_types::resolver::resolve_unqualified_access(
            NodeRef::new(FileId(1), ctx.tree(pkl_types::FileId(1)).unwrap().root()),
            "output",
            ctx,
        )
        .unwrap();
        assert_eq!(output.node.file, FileId(2));
        assert_eq!(
            pkl_types::resolver::resolve_unqualified_access(
                NodeRef::new(FileId(1), ctx.tree(pkl_types::FileId(1)).unwrap().root()),
                "z",
                ctx
            )
            .unwrap()
            .ty,
            *ctx.base.int_type()
        );
        let offset = harness::offset(ctx.tree(pkl_types::FileId(1)).unwrap(), 5, 6);
        let response = lsp_features::completion::completion(
            ctx,
            pkl_types::InFile::new(pkl_types::FileId(1), offset),
            |range| harness::range(ctx.tree(pkl_types::FileId(1)).unwrap(), range),
        )
        .unwrap();
        let items = match response {
            CompletionResponse::Array(v) => v,
            CompletionResponse::List(v) => v.items,
        };
        let item = items.iter().find(|i| i.label == "output").unwrap();
        assert_eq!(item.kind, Some(lsp_types::CompletionItemKind::MODULE));
        assert!(labels(ctx, 5, 13).contains(&"value".to_owned()));
    });
}
#[test]
fn imported_alias_members_have_types_and_completions() {
    with_consumer_regressions(|ctx| {
        assert_eq!(
            pkl_types::resolver::resolve_unqualified_access(
                NodeRef::new(FileId(1), ctx.tree(pkl_types::FileId(1)).unwrap().root()),
                "q",
                ctx
            )
            .unwrap()
            .ty,
            *ctx.base.int_type()
        );
        let library = pkl_types::resolver::resolve_unqualified_access(
            NodeRef::new(FileId(1), ctx.tree(pkl_types::FileId(1)).unwrap().root()),
            "library",
            ctx,
        )
        .unwrap();
        let alias = pkl_types::find_member(&library.ty, "Alias", ctx).unwrap();
        assert!(matches!(alias.ty, pkl_types::Type::Alias { .. }));
        assert_eq!(alias.node.file, FileId(2));
        assert!(labels(ctx, 4, 7).contains(&"field".to_owned()));
    });
}
#[test]
fn imported_argument_context_and_let_initializer_use_outer_scope() {
    with_consumer_regressions(|ctx| {
        assert_eq!(
            pkl_types::resolver::resolve_unqualified_access(
                NodeRef::new(FileId(1), ctx.tree(pkl_types::FileId(1)).unwrap().root()),
                "y",
                ctx
            )
            .unwrap()
            .ty,
            *ctx.base.int_type()
        );
        fn argument(
            t: &pkl_syntax::ast::SyntaxTree,
            n: pkl_syntax::ast::NodeId,
        ) -> Option<pkl_syntax::ast::NodeId> {
            if matches!(t.kind(n), NodeKind::UnqualifiedAccessExpr) && t.text(n) == "foobar" {
                return Some(n);
            }
            t.children(n).iter().find_map(|&c| argument(t, c))
        }
        let arg = argument(
            ctx.tree(pkl_types::FileId(1)).unwrap(),
            ctx.tree(pkl_types::FileId(1)).unwrap().root(),
        )
        .unwrap();
        assert_eq!(
            NodeRef::new(FileId(1), arg)
                .cast(ctx, pkl_syntax::ast::nodes::Expr::cast)
                .map_or(pkl_types::Type::Unknown, |expr| pkl_types::expected_type(
                    expr, ctx
                )),
            *ctx.base.string_type()
        );
        assert!(labels(ctx, 6, 29).contains(&"value".to_owned()));
    });
}

#[test]
fn glob_uri_navigates_to_all_pinned_modules_and_empty_glob_has_no_definition() {
    let base = harness::load_test_base();
    let tree =
        pkl_syntax::parse("import* \"*.pkl\" as libs\nx = import*(\"empty*.pkl\")\n").unwrap();
    let first = Arc::new(pkl_syntax::parse("a = 1\n").unwrap());
    let second = Arc::new(pkl_syntax::parse("b = 2\n").unwrap());
    let targets = vec![
        NodeRef::new(FileId(2), first.root()),
        NodeRef::new(FileId(3), second.root()),
    ];
    let import = find(&tree, tree.root(), &NodeKind::ImportGlob).unwrap();
    let expression = find(&tree, tree.root(), &NodeKind::ImportExpr).unwrap();
    let edges = HashMap::from([
        (
            NodeRef::new(FileId(1), import),
            ImportResolution::Glob(targets.clone()),
        ),
        (
            NodeRef::new(FileId(1), expression),
            ImportResolution::Glob(vec![]),
        ),
    ]);
    let trees = HashMap::from([
        (FileId(1), std::sync::Arc::new(tree)),
        (base.file_id(), base.tree().clone()),
        (FileId(2), first),
        (FileId(3), second),
    ]);
    let ctx = AnalysisContext::new(&base, &trees, &edges);
    let callback = |id: FileId| format!("file:///{}.pkl", id.0).parse().ok();
    let GotoDefinitionResponse::Array(locations) = lsp_features::goto_definition::goto_definition(
        &ctx,
        pkl_types::InFile::new(
            pkl_types::FileId(1),
            harness::offset(ctx.tree(pkl_types::FileId(1)).unwrap(), 0, 10),
        ),
        callback,
        harness::range,
    )
    .unwrap() else {
        panic!("glob array");
    };
    assert_eq!(locations.len(), 2);
    for (location, target) in locations.iter().zip(&targets) {
        assert_eq!(location.uri, callback(target.file).unwrap());
        assert_eq!(
            location.range,
            harness::range(
                ctx.tree(target.file).unwrap(),
                ctx.tree(target.file).unwrap().span(target.value)
            )
        );
    }
    assert!(
        lsp_features::goto_definition::goto_definition(
            &ctx,
            pkl_types::InFile::new(
                pkl_types::FileId(1),
                harness::offset(ctx.tree(pkl_types::FileId(1)).unwrap(), 1, 16)
            ),
            callback,
            harness::range
        )
        .is_none()
    );
    assert!(
        lsp_features::goto_definition::goto_definition(
            &ctx,
            pkl_types::InFile::new(
                pkl_types::FileId(1),
                harness::offset(ctx.tree(pkl_types::FileId(1)).unwrap(), 0, 20)
            ),
            callback,
            harness::range
        )
        .is_none()
    );
}
#[test]
fn editor_requests_select_the_qualified_file_and_do_not_fall_back() {
    let base = harness::load_test_base();
    let integer = Arc::new(pkl_syntax::parse("x = 1\ny = x\n").unwrap());
    let string = Arc::new(pkl_syntax::parse("x: String = \"text\"\ny = x\n").unwrap());
    let file = FileId(7);
    let trees = HashMap::from([
        (base.file_id(), base.tree().clone()),
        (FileId(6), integer),
        (file, string.clone()),
    ]);
    let imports = HashMap::new();
    let ctx = AnalysisContext::new(&base, &trees, &imports);
    let offset = pkl_types::InFile::new(file, harness::offset(&string, 1, 4));
    let hover = lsp_features::hover::hover(&ctx, offset).unwrap();
    let HoverContents::Markup(markdown) = hover.contents else {
        panic!("markdown")
    };
    assert!(markdown.value.contains("x: String"), "{}", markdown.value);
    let completion =
        lsp_features::completion::completion(&ctx, offset, |range| harness::range(&string, range))
            .unwrap();
    let items = match completion {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(list) => list.items,
    };
    assert_eq!(
        items
            .iter()
            .find(|item| item.label == "x")
            .unwrap()
            .detail
            .as_deref(),
        Some("String")
    );
    let definition = lsp_features::goto_definition::goto_definition(
        &ctx,
        offset,
        |id| (id == file).then(|| "file:///string.pkl".parse().unwrap()),
        harness::range,
    )
    .unwrap();
    let GotoDefinitionResponse::Scalar(location) = definition else {
        panic!("location")
    };
    let property = find(&string, string.root(), &NodeKind::ClassProperty).unwrap();
    assert_eq!(
        location.range,
        harness::range(&string, string.span(property))
    );

    let missing = pkl_types::InFile::new(FileId(99), offset.value);
    assert!(lsp_features::hover::hover(&ctx, missing).is_none());
    assert!(
        lsp_features::completion::completion(&ctx, missing, |range| harness::range(&string, range))
            .is_none()
    );
    assert!(
        lsp_features::goto_definition::goto_definition(&ctx, missing, |_| None, harness::range)
            .is_none()
    );
}

#[test]
fn generic_class_completion_details_use_instantiated_member_types() {
    let base = harness::load_test_base();
    harness::with_snippet(
        &base,
        "class Box<T> { value: T }\nbox: Box<String>\nx = box.value\n",
        |tree, ctx| {
            let response = lsp_features::completion::completion(
                ctx,
                pkl_types::InFile::new(FileId(1), harness::offset(tree, 2, 8)),
                |range| harness::range(tree, range),
            )
            .unwrap();
            let items = match response {
                CompletionResponse::Array(items) => items,
                CompletionResponse::List(list) => list.items,
            };
            let member = items.iter().find(|item| item.label == "value").unwrap();
            assert_eq!(member.detail.as_deref(), Some("String"));
        },
    );
}
