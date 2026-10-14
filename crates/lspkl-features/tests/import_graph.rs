mod harness;
use lsp_types::{CompletionResponse, GotoDefinitionResponse, HoverContents};
use lspkl_db::File;
use lspkl_syntax::ast::NodeKind;
use lspkl_types::{AnalysisContext, NodeRef};
use std::sync::Arc;
fn with_graph(f: impl FnOnce(&AnalysisContext)) {
    let base = harness::load_test_base();
    let current=Arc::new(lspkl_syntax::parse("extends \"3.pkl\"\nimport \"2.pkl\" as library\nimport* \"2*.pkl\" as libs\nx = library.value\ny = inherited\nz = libs[\"2.pkl\"].value\n").unwrap());
    let lib = Arc::new(
        lspkl_syntax::parse("/// Imported docs\nvalue = 42\nclass SomeClass {}\n").unwrap(),
    );
    let parent = Arc::new(lspkl_syntax::parse("inherited: String = \"yes\"\n").unwrap());
    base.add(1, current.source());
    base.add(2, lib.source());
    base.add(3, parent.source());
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, lspkl_db::Db::file(&*db, &harness::uri(1)).unwrap());
    f(&ctx);
}
fn find(
    t: &lspkl_syntax::ast::SyntaxTree,
    n: lspkl_syntax::ast::NodeId,
    k: &NodeKind,
) -> Option<lspkl_syntax::ast::NodeId> {
    if t.kind(n) == k {
        return Some(n);
    }
    t.children(n).iter().find_map(|&c| find(t, c, k))
}
fn labels(ctx: &AnalysisContext, line: u32, col: u32) -> Vec<String> {
    let offset = harness::offset(ctx.tree(harness::file(ctx, 1)).unwrap(), line, col);
    match lspkl_features::completion::completion(
        ctx.db,
        ctx.root,
        lspkl_types::InFile::new(harness::file(ctx, 1), offset),
        |range| harness::range(ctx.tree(harness::file(ctx, 1)).unwrap(), range),
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
        let h = lspkl_features::hover::hover(
            ctx.db,
            ctx.root,
            lspkl_types::InFile::new(
                harness::file(ctx, 1),
                harness::offset(ctx.tree(harness::file(ctx, 1)).unwrap(), 3, 13),
            ),
        )
        .unwrap();
        let HoverContents::Markup(m) = h.contents else {
            panic!("markdown")
        };
        assert!(m.value.contains("value: Int"));
        assert!(m.value.contains("Imported docs"));
        let d = lspkl_features::goto_definition::goto_definition(
            ctx.db,
            ctx.root,
            lspkl_types::InFile::new(
                harness::file(ctx, 1),
                harness::offset(ctx.tree(harness::file(ctx, 1)).unwrap(), 3, 13),
            ),
            |id| (id == harness::file(ctx, 2)).then(|| harness::uri(2).as_str().parse().unwrap()),
            harness::range,
        )
        .unwrap();
        let GotoDefinitionResponse::Scalar(l) = d else {
            panic!("location")
        };
        let target = ctx.tree(harness::file(ctx, 2)).unwrap();
        let property = find(target, target.root(), &NodeKind::ClassProperty).unwrap();
        assert_eq!(l.range, harness::range(target, target.span(property)));
        assert_eq!(l.uri.as_str(), harness::uri(2).as_str());
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
        let glob = labels(ctx, 5, 23);
        assert!(glob.contains(&"value".into()), "{glob:?}");
    });
}

fn with_consumer_regressions(f: impl FnOnce(&AnalysisContext)) {
    let base = harness::load_test_base();
    let current=Arc::new(lspkl_syntax::parse("extends \"3.pkl\"\nimport \"2.pkl\" as library\nimport \"2.pkl\" as output\nx: library.Alias\nq = x.field\nz = output.value\ny = let (library = library.value) library\nw = library.f(foobar)\n").unwrap());
    let lib=Arc::new(lspkl_syntax::parse("class C { field: Int }\ntypealias Alias = C\nvalue = 42\nfunction f(arg: String): String = arg\n").unwrap());
    let parent = Arc::new(lspkl_syntax::parse("output: String = \"parent\"\n").unwrap());
    base.add(1, current.source());
    base.add(2, lib.source());
    base.add(3, parent.source());
    let db = base.db.borrow();
    f(&AnalysisContext::for_root(
        &*db,
        lspkl_db::Db::file(&*db, &harness::uri(1)).unwrap(),
    ));
}
#[test]
fn lexical_import_precedes_inherited_output_in_completion() {
    with_consumer_regressions(|ctx| {
        let offset = harness::offset(ctx.tree(harness::file(ctx, 1)).unwrap(), 5, 6);
        let response = lspkl_features::completion::completion(
            ctx.db,
            ctx.root,
            lspkl_types::InFile::new(harness::file(ctx, 1), offset),
            |range| harness::range(ctx.tree(harness::file(ctx, 1)).unwrap(), range),
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
fn imported_alias_members_have_completions() {
    with_consumer_regressions(|ctx| {
        assert!(labels(ctx, 4, 7).contains(&"field".to_owned()));
    });
}
#[test]
fn imported_let_initializer_completes_outer_import_members() {
    with_consumer_regressions(|ctx| {
        assert!(labels(ctx, 6, 29).contains(&"value".to_owned()));
    });
}

#[test]
fn glob_uri_navigates_to_all_pinned_modules_and_empty_glob_has_no_definition() {
    let base = harness::load_test_base();
    let tree =
        lspkl_syntax::parse("import* \"[23].pkl\" as libs\nx = import*(\"empty*.pkl\")\n").unwrap();
    let first = Arc::new(lspkl_syntax::parse("a = 1\n").unwrap());
    let second = Arc::new(lspkl_syntax::parse("b = 2\n").unwrap());
    base.add(1, tree.source());
    base.add(2, first.source());
    base.add(3, second.source());
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, lspkl_db::Db::file(&*db, &harness::uri(1)).unwrap());
    let targets = vec![
        NodeRef::new(harness::file(&ctx, 2), first.root()),
        NodeRef::new(harness::file(&ctx, 3), second.root()),
    ];
    let callback = |id: File| id.uri(&*db).as_str().parse().ok();
    let GotoDefinitionResponse::Array(locations) =
        lspkl_features::goto_definition::goto_definition(
            ctx.db,
            ctx.root,
            lspkl_types::InFile::new(
                harness::file(&ctx, 1),
                harness::offset(ctx.tree(harness::file(&ctx, 1)).unwrap(), 0, 10),
            ),
            callback,
            harness::range,
        )
        .unwrap()
    else {
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
        lspkl_features::goto_definition::goto_definition(
            ctx.db,
            ctx.root,
            lspkl_types::InFile::new(
                harness::file(&ctx, 1),
                harness::offset(ctx.tree(harness::file(&ctx, 1)).unwrap(), 1, 16)
            ),
            callback,
            harness::range
        )
        .is_none()
    );
    assert!(
        lspkl_features::goto_definition::goto_definition(
            ctx.db,
            ctx.root,
            lspkl_types::InFile::new(
                harness::file(&ctx, 1),
                harness::offset(ctx.tree(harness::file(&ctx, 1)).unwrap(), 0, 20)
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
    let integer = Arc::new(lspkl_syntax::parse("x = 1\ny = x\n").unwrap());
    let string = Arc::new(lspkl_syntax::parse("x: String = \"text\"\ny = x\n").unwrap());
    base.add(6, integer.source());
    let file = base.add(7, string.source());
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, lspkl_db::Db::file(&*db, &harness::uri(1)).unwrap());
    let offset = lspkl_types::InFile::new(file, harness::offset(&string, 1, 4));
    let hover = lspkl_features::hover::hover(ctx.db, ctx.root, offset).unwrap();
    let HoverContents::Markup(markdown) = hover.contents else {
        panic!("markdown")
    };
    assert!(markdown.value.contains("x: String"), "{}", markdown.value);
    let completion = lspkl_features::completion::completion(ctx.db, ctx.root, offset, |range| {
        harness::range(&string, range)
    })
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
    let definition = lspkl_features::goto_definition::goto_definition(
        ctx.db,
        ctx.root,
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

    let missing = lspkl_types::InFile::new(harness::file(&ctx, 99), offset.value);
    assert!(lspkl_features::hover::hover(ctx.db, ctx.root, missing).is_none());
    assert!(
        lspkl_features::completion::completion(ctx.db, ctx.root, missing, |range| harness::range(
            &string, range
        ))
        .is_none()
    );
    assert!(
        lspkl_features::goto_definition::goto_definition(
            ctx.db,
            ctx.root,
            missing,
            |_| None,
            harness::range
        )
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
            let response = lspkl_features::completion::completion(
                ctx.db,
                ctx.root,
                lspkl_types::InFile::new(harness::file(ctx, 1), harness::offset(tree, 2, 8)),
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
