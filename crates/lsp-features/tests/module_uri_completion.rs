mod harness;
use lsp_features::module_uri_completion::{completion, resolve};
use lsp_features::positions::PositionEncoding;
use lsp_types::{CompletionItem, CompletionResponse, CompletionTextEdit, Documentation};
use pkl_project::import_resolver::ImportResolver;
use pkl_syntax::TextSize;

#[test]
fn uri_owners_use_explicit_db_and_encoding_with_lazy_metadata() {
    let base = harness::load_test_base();
    for marked in [
        "import \"pkl:ba|\"",
        "extends \"pkl:ba|\"",
        "amends \"pkl:ba|\"",
        "x = import(\"pkl:ba|\")",
        "import #\"pkl:ba|\"#",
    ] {
        let cursor = marked.find('|').unwrap();
        let file = base.add(1, &marked.replace('|', ""));
        let db = base.db.borrow();
        let loader = pkl_db::DatabaseSourceLoader::new(&*db);
        let resolver = ImportResolver::new(&loader, db.config().cache_dir(&*db));
        for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
            let CompletionResponse::Array(items) = completion(
                &*db,
                file,
                TextSize::try_from(cursor).unwrap(),
                encoding,
                &resolver,
            )
            .unwrap() else {
                panic!("array")
            };
            let item = items
                .into_iter()
                .find(|item| item.label == "pkl:base")
                .unwrap();
            assert!(item.documentation.is_none());
            let data = item.data.as_ref().unwrap();
            assert_eq!(data["moduleUriCompletion"], true);
            assert_eq!(data["targetUri"], "pkl:base");
            assert_eq!(data["originUri"], "file:///1.pkl");
            assert!(data.get("originRange").is_some());
            let CompletionTextEdit::Edit(edit) = item.text_edit.as_ref().unwrap() else {
                panic!("edit")
            };
            assert_eq!(edit.new_text, "pkl:base");
            let resolved = resolve(&*db, item);
            let Documentation::MarkupContent(docs) = resolved.documentation.unwrap() else {
                panic!("markdown")
            };
            assert!(docs.value.contains("```pkl"));
        }
    }
}

#[test]
fn non_uri_literals_and_glob_stdlib_do_not_complete() {
    let base = harness::load_test_base();
    for (marked, absent) in [("x = \"pkl:ba|\"", true), ("import* \"pkl:ba|\"", false)] {
        let cursor = marked.find('|').unwrap();
        let file = base.add(1, &marked.replace('|', ""));
        let db = base.db.borrow();
        let loader = pkl_db::DatabaseSourceLoader::new(&*db);
        let resolver = ImportResolver::new(&loader, db.config().cache_dir(&*db));
        let result = completion(
            &*db,
            file,
            TextSize::try_from(cursor).unwrap(),
            PositionEncoding::Utf16,
            &resolver,
        );
        if absent {
            assert!(result.is_none());
        } else {
            let Some(CompletionResponse::Array(items)) = result else {
                panic!("array")
            };
            assert!(items.is_empty());
        }
    }
    let item = CompletionItem {
        label: "unrelated".into(),
        ..Default::default()
    };
    assert_eq!(resolve(&*base.db.borrow(), item.clone()), item);
}

use pkl_db::Db;
