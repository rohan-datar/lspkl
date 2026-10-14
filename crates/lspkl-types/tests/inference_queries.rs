use fluent_uri::Uri;
use lspkl_db::{File, FileStatus, RootDatabase};
use lspkl_syntax::ast::{NodeId, NodeKind};
use lspkl_types::{DeclId, ItemId, Type, base_module, decl_type, infer, module_items};
use std::sync::Arc;

fn put(db: &mut RootDatabase, text: &str) -> File {
    db.upsert_file(
        &Uri::parse("file:///inference_queries.pkl".to_owned()).unwrap(),
        Arc::from(text),
        None,
        FileStatus::Present,
    )
    .unwrap()
}
fn property(db: &RootDatabase, file: File, name: &str) -> DeclId {
    let item = module_items(db, file)
        .properties
        .iter()
        .find(|(_, property)| property.declaration.name.text(db) == name)
        .unwrap()
        .0;
    DeclId {
        file,
        root: file,
        item: ItemId::Property(item),
    }
}

#[test]
fn cyclic_properties_recover_and_recompute_after_the_cycle_is_broken() {
    let mut db = RootDatabase::default();
    let file = put(&mut db, "a = b\nb = a\nindependent = 42\n");
    let a = property(&db, file, "a");
    let b = property(&db, file, "b");
    assert_eq!(decl_type(&db, a), Type::Unknown);
    assert_eq!(decl_type(&db, b), Type::Unknown);
    assert_eq!(
        decl_type(&db, property(&db, file, "independent")),
        *base_module(&db).int_type()
    );
    let body = infer(&db, a);
    let tree = lspkl_db::parse(&db, file).as_ref().unwrap();
    let value = tree
        .child(NodeId::ROOT, |kind| matches!(kind, NodeKind::ClassProperty))
        .unwrap();
    let reference = tree
        .child(value, |kind| {
            matches!(kind, NodeKind::UnqualifiedAccessExpr)
        })
        .unwrap();
    assert_eq!(body.expression_types.get(&reference), Some(&Type::Unknown));
    put(&mut db, "a = \"resolved\"\nb = a\nindependent = 42\n");
    assert_eq!(decl_type(&db, a), Type::string_literal("resolved"));
    assert_eq!(decl_type(&db, b), Type::string_literal("resolved"));
}

#[test]
fn local_object_dependencies_do_not_cycle_the_entire_declaration_body() {
    let mut db = RootDatabase::default();
    let file = put(
        &mut db,
        "object = new { local first = 42; local second = first; local cycle = cycle }\n",
    );
    let object = property(&db, file, "object");
    let body = infer(&db, object);
    let tree = lspkl_db::parse(&db, file).as_ref().unwrap();
    let mut pending = vec![tree.root()];
    let mut found = std::collections::HashMap::new();
    while let Some(node) = pending.pop() {
        if matches!(tree.kind(node), NodeKind::UnqualifiedAccessExpr) {
            found.insert(
                tree.name(node).unwrap(),
                body.expression_types.get(&node).unwrap(),
            );
        }
        pending.extend(tree.children(node));
    }
    assert_eq!(found.get("first"), Some(&base_module(&db).int_type()));
    assert_eq!(found.get("cycle"), Some(&&Type::Unknown));
}

#[test]
fn body_edits_update_inferred_types_without_changing_annotated_declarations() {
    let mut db = RootDatabase::default();
    let file = put(&mut db, "value = 42\ntyped: String = \"before\"\n");
    let value = property(&db, file, "value");
    let typed = property(&db, file, "typed");
    assert_eq!(decl_type(&db, value), *base_module(&db).int_type());
    assert_eq!(decl_type(&db, typed), *base_module(&db).string_type());
    put(
        &mut db,
        "value = \"after\"\ntyped: String = \"different\"\n",
    );
    assert_eq!(decl_type(&db, value), Type::string_literal("after"));
    assert_eq!(decl_type(&db, typed), *base_module(&db).string_type());
}
