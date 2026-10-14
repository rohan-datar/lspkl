#[path = "../../pkl-db/tests/support/mod.rs"]
mod input_support;
use fluent_uri::Uri;
use parking_lot::Mutex;
use pkl_db::{File, FileStatus, RootDatabase};
use pkl_types::{
    ClassId, DeclId, ItemId, Modifiers, ModuleId, TypeRef, class_members, decl_for_node, item_node,
    item_source_map, module_items, module_members,
};
use pkl_vfs::StandardSourceLoader;
use std::sync::Arc;

fn put(db: &mut RootDatabase, name: &str, text: &str) -> File {
    db.upsert_file(
        &Uri::parse(format!("file:///phase7/{name}.pkl")).unwrap(),
        Arc::from(text),
        None,
        FileStatus::Present,
    )
    .unwrap()
}
fn class(db: &RootDatabase, file: File, root: File, name: &str) -> ClassId {
    let index = module_items(db, file)
        .classes
        .iter()
        .find(|(_, c)| c.declaration.name.text(db) == name)
        .unwrap()
        .0;
    ClassId::new(db, ModuleId::new(db, file, root), index)
}
fn names(db: &RootDatabase, class: ClassId) -> Vec<String> {
    class_members(db, class)
        .entries
        .iter()
        .map(|m| m.name.text(db).clone())
        .collect()
}
#[test]
fn value_and_body_edits_stop_at_the_declaration_firewall() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let log = events.clone();
    let mut db = RootDatabase::with_event_callback(
        Arc::new(StandardSourceLoader::default()),
        Default::default(),
        Some(Box::new(move |event| {
            if let salsa::EventKind::WillExecute { database_key } = event.kind {
                log.lock().push(format!("{database_key:?}"));
            }
        })),
    );
    let a = put(
        &mut db,
        "a",
        "open class A {\n x: Int = 1\n function f(): Int = 2\n y: Int = 3\n}\n",
    );
    let b = put(
        &mut db,
        "b",
        "import \"a.pkl\"\nclass B extends a.A { z: String = \"z\" }\n",
    );
    let b_class = class(&db, b, b, "B");
    let before = class_members(&db, b_class).clone();
    assert_eq!(names(&db, b_class), ["z", "x", "f", "y"]);
    let source = item_source_map(&db, a).clone();
    let y = module_items(&db, a)
        .properties
        .iter()
        .find(|(_, p)| p.declaration.name.text(&db) == "y")
        .unwrap()
        .0;
    let decl = DeclId {
        file: a,
        root: a,
        item: ItemId::Property(y),
    };
    let old_node = item_node(&db, decl).unwrap();
    events.lock().clear();
    put(
        &mut db,
        "a",
        "open class A {\n x: Int = 1 + 20\n function f(): Int = if (true) 5 else 9\n y: Int = 3\n}\n",
    );
    assert_eq!(class_members(&db, b_class), &before);
    assert_eq!(module_items(&db, b).classes.len(), 1);
    assert_ne!(item_source_map(&db, a), &source);
    let new_node = item_node(&db, decl).unwrap();
    assert_ne!(old_node, new_node);
    assert_eq!(decl_for_node(&db, new_node, a), Some(decl));
    let executions = events.lock();
    assert!(
        executions.iter().any(|e| e.contains("item_source_map")),
        "{executions:?}"
    );
    assert!(
        executions.iter().any(|e| e.contains("declaration_data")),
        "{executions:?}"
    );
    assert!(
        !executions
            .iter()
            .any(|e| e.contains("module_items") || e.contains("class_members")),
        "{executions:?}"
    );
}
#[test]
fn annotations_names_and_inheritance_refresh_importing_members() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let log = events.clone();
    let mut db = RootDatabase::with_event_callback(
        Arc::new(StandardSourceLoader::default()),
        Default::default(),
        Some(Box::new(move |event| {
            if let salsa::EventKind::WillExecute { database_key } = event.kind {
                log.lock().push(format!("{database_key:?}"));
            }
        })),
    );
    let a = put(
        &mut db,
        "a",
        "open class A { x: Int = 1 }\nopen class Other { q: Int = 2 }",
    );
    let b = put(
        &mut db,
        "b",
        "import \"a.pkl\"\nclass B extends a.A { own: Int = 3 }",
    );
    let b_class = class(&db, b, b, "B");
    let original = module_items(&db, a).clone();
    assert_eq!(names(&db, b_class), ["own", "x"]);
    events.lock().clear();
    put(
        &mut db,
        "a",
        "open class A { x: String = \"new\" }\nopen class Other { q: Int = 2 }",
    );
    assert_ne!(module_items(&db, a), &original);
    let items = module_items(&db, a);
    assert!(matches!(
        items.properties.iter().next().unwrap().1.ty,
        Some(TypeRef::Named { .. })
    ));
    assert_eq!(names(&db, b_class), ["own", "x"]);
    assert!(events.lock().iter().any(|e| e.contains("class_members")));
    assert!(events.lock().iter().any(|e| e.contains("module_items")));
    put(
        &mut db,
        "a",
        "open class A { renamed: String = \"new\" }\nopen class Other { q: Int = 2 }",
    );
    assert_eq!(names(&db, b_class), ["own", "renamed"]);
    put(
        &mut db,
        "b",
        "import \"a.pkl\"\nclass B extends a.Other { own: Int = 3 }",
    );
    assert_eq!(names(&db, b_class), ["own", "q"]);
}
#[test]
fn cycles_overrides_and_supermodules_are_finite() {
    let mut db = RootDatabase::new(
        Arc::new(StandardSourceLoader::default()),
        Default::default(),
    );
    let file = put(
        &mut db,
        "cycles",
        "open class A extends B { a: Int = 1\n same: Int = 2 }\nopen class B extends A { b: Int = 3\n same: String = \"b\" }\ntypealias X = Y\ntypealias Y = X\nclass C extends X { c: Int = 1 }",
    );
    let a = class(&db, file, file, "A");
    assert_eq!(names(&db, a), ["a", "same", "b"]);
    assert_eq!(class_members(&db, a).own_len, 2);
    assert_eq!(names(&db, class(&db, file, file, "C")), ["c"]);
    let left = put(
        &mut db,
        "left",
        "extends \"right.pkl\"\nlocal private: Int = 1\nleft: Int = 2\nsame: Int = 3",
    );
    put(
        &mut db,
        "right",
        "extends \"left.pkl\"\nright: Int = 4\nsame: Int = 5",
    );
    let table = module_members(&db, ModuleId::new(&db, left, left));
    assert_eq!(table.own_len, 3);
    let names: Vec<_> = table
        .entries
        .iter()
        .map(|e| e.name.text(&db).as_str())
        .collect();
    assert_eq!(&names[..4], ["private", "left", "same", "right"]);
    assert_eq!(names.iter().filter(|&&n| n == "same").count(), 1);
    assert!(names.contains(&"getClass"));
}
#[test]
fn lowering_covers_all_annotation_shapes_and_modifiers() {
    let mut db = RootDatabase::new(
        Arc::new(StandardSourceLoader::default()),
        Default::default(),
    );
    let file = put(
        &mut db,
        "types",
        "import \"a.pkl\" as alias\nopen class Box<out T> {\n hidden fixed value: alias.A<String>? = null\n function f<U>(arg: (Int | String), other: unknown): (Int, String) -> Boolean = (x,y) -> true\n}\ntypealias Choice = *\"yes\" | \"no\"\ntypealias Constrained = Int(this > 0)\na: nothing\nb: module",
    );
    let items = module_items(&db, file);
    let property = items
        .properties
        .iter()
        .find(|(_, p)| p.declaration.name.text(&db) == "value")
        .unwrap()
        .1;
    assert!(
        property
            .declaration
            .modifiers
            .contains(Modifiers::HIDDEN | Modifiers::FIXED)
    );
    assert!(matches!(property.ty, Some(TypeRef::Nullable(_))));
    let method = items.methods.iter().next().unwrap().1;
    assert_eq!(method.type_parameters.len(), 1);
    assert_eq!(method.parameters.len(), 2);
    assert!(matches!(method.result, Some(TypeRef::Function { .. })));
    assert!(matches!(
        items
            .type_aliases
            .iter()
            .find(|(_, a)| a.declaration.name.text(&db) == "Choice")
            .unwrap()
            .1
            .ty,
        TypeRef::Union(..)
    ));
    assert!(matches!(
        items
            .type_aliases
            .iter()
            .find(|(_, a)| a.declaration.name.text(&db) == "Constrained")
            .unwrap()
            .1
            .ty,
        TypeRef::Constrained { .. }
    ));
    assert!(
        items
            .properties
            .iter()
            .any(|(_, p)| p.ty == Some(TypeRef::Nothing))
    );
    assert!(
        items
            .properties
            .iter()
            .any(|(_, p)| p.ty == Some(TypeRef::Module))
    );
}

#[test]
fn variance_and_constraint_shape_changes_cross_the_declaration_firewall() {
    use pkl_syntax::ast::nodes::Variance;
    let mut db = RootDatabase::new(
        Arc::new(StandardSourceLoader::default()),
        Default::default(),
    );
    let file = put(&mut db, "variance", "class Box<out/* comment */ T> {}");
    let before = module_items(&db, file).clone();
    assert_eq!(
        before.classes.iter().next().unwrap().1.type_parameters[0].variance,
        Variance::Covariant
    );
    put(&mut db, "variance", "class Box<in T> {}");
    let after = module_items(&db, file);
    assert_ne!(after, &before);
    assert_eq!(
        after.classes.iter().next().unwrap().1.type_parameters[0].variance,
        Variance::Contravariant
    );
    let file = put(
        &mut db,
        "constraints",
        r#"typealias Refined = Int(foo["x"] == 1)"#,
    );
    let before = module_items(&db, file).clone();
    put(
        &mut db,
        "constraints",
        r#"typealias Refined = Int(foo("x") == 1)"#,
    );
    assert_ne!(module_items(&db, file), &before);
}

#[test]
fn qualified_inheritance_uses_uri_aliases_with_queries_and_repeated_suffixes() {
    let mut db = RootDatabase::new(
        Arc::new(StandardSourceLoader::default()),
        Default::default(),
    );
    let uri = Uri::parse("https://example.test/a.pkl.pkl?download=1#note".to_owned()).unwrap();
    let imported = db
        .upsert_file(
            &uri,
            Arc::from("open class A { inherited: Int = 1 }"),
            None,
            FileStatus::Present,
        )
        .unwrap();
    let root = put(
        &mut db,
        "uri_alias",
        r#"import "https://example.test/a.pkl.pkl?download=1#note"
class B extends `a.pkl`.A { own: Int = 2 }"#,
    );
    let members = class_members(&db, class(&db, root, root, "B"));
    let inherited = members
        .entries
        .iter()
        .find(|member| member.name.text(&db) == "inherited")
        .unwrap();
    assert_eq!(inherited.declaration.file, imported);
    let mapped = item_node(&db, inherited.declaration).unwrap();
    assert_eq!(
        decl_for_node(&db, mapped, root),
        Some(inherited.declaration)
    );
}

#[test]
fn qualified_types_do_not_leak_the_imported_modules_lexical_base_namespace() {
    let mut db = RootDatabase::new(
        Arc::new(StandardSourceLoader::default()),
        Default::default(),
    );
    put(
        &mut db,
        "namespace",
        "marker: Int = 1\ntypealias Output = FileOutput\n",
    );
    let file = put(
        &mut db,
        "qualified",
        "import \"namespace.pkl\"\nclass Missing extends namespace.FileOutput { own: Int = 1 }\nclass Found extends namespace.Output {}\n",
    );
    assert_eq!(names(&db, class(&db, file, file, "Missing")), ["own"]);
    let found = class_members(&db, class(&db, file, file, "Found"));
    let value = found
        .entries
        .iter()
        .find(|member| member.name.text(&db) == "value")
        .unwrap();
    let base = pkl_db::Db::file(&db, &Uri::parse("pkl:base".to_owned()).unwrap()).unwrap();
    assert_eq!(value.declaration.file, base);
}

#[test]
fn foreign_inheritance_uses_the_request_roots_nearest_local_project() {
    fn project(root: &std::path::Path, modulepath: &str) -> Arc<pkl_project::PklProject> {
        Arc::new(pkl_project::PklProject {
            metadata: pkl_project::DerivedProjectMetadata {
                project_file_uri: pkl_vfs::path_to_uri(&root.join("PklProject"))
                    .unwrap()
                    .to_string(),
                package_uri: None,
                declared_dependencies: Default::default(),
                evaluator_settings: Some(pkl_project::EvaluatorSettings {
                    module_cache_dir: None,
                    module_path: Some(vec![modulepath.to_owned()]),
                }),
            },
            project_deps: None,
        })
    }
    fn local(db: &mut RootDatabase, path: &std::path::Path, text: &str) -> File {
        db.upsert_file(
            &pkl_vfs::path_to_uri(path).unwrap(),
            Arc::from(text),
            None,
            FileStatus::Present,
        )
        .unwrap()
    }
    let root = tempfile::tempdir().unwrap();
    let inner = root.path().join("inner");
    let mut db = RootDatabase::new(
        Arc::new(StandardSourceLoader::default()),
        Default::default(),
    );
    db.set_projects(vec![
        project(root.path(), "modules"),
        project(&inner, "modules"),
    ]);
    local(
        &mut db,
        &root.path().join("modules/parent.pkl"),
        "outerValue: Int = 1",
    );
    local(&mut db, &inner.join("modules/parent.pkl"), "inner: Int = 2");
    let remote = db
        .open_file(
            &Uri::parse("https://example.test/remote.pkl".to_owned()).unwrap(),
            Arc::from("extends \"modulepath:/parent.pkl\"\nremote: Int = 3"),
            None,
        )
        .unwrap();
    let child = local(
        &mut db,
        &inner.join("child.pkl"),
        "import \"https://example.test/remote.pkl\"",
    );
    let main = local(
        &mut db,
        &root.path().join("main.pkl"),
        "import \"inner/child.pkl\"\nimport \"https://example.test/remote.pkl\"",
    );
    let outer = local(
        &mut db,
        &root.path().join("outer.pkl"),
        "import \"https://example.test/remote.pkl\"",
    );
    input_support::preload(&mut db);
    let child_project = pkl_db::contextual_project(&db, main, child);
    assert_eq!(pkl_db::contextual_project(&db, main, remote), child_project);
    let inner_members = module_members(&db, ModuleId::new(&db, remote, main));
    assert!(
        inner_members
            .entries
            .iter()
            .any(|e| e.name.text(&db) == "inner")
    );
    assert!(
        !inner_members
            .entries
            .iter()
            .any(|e| e.name.text(&db) == "outerValue")
    );
    let outer_members = module_members(&db, ModuleId::new(&db, remote, outer));
    assert!(
        outer_members
            .entries
            .iter()
            .any(|e| e.name.text(&db) == "outerValue")
    );
    assert!(
        !outer_members
            .entries
            .iter()
            .any(|e| e.name.text(&db) == "inner")
    );
}
