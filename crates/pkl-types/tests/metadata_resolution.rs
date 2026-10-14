use fluent_uri::Uri;
use pkl_db::{Db, File, FileStatus, RootDatabase};
use pkl_types::{
    ClassId, DeclId, ItemId, Name, Type, TypeRef, base_module, decl_type, find_member, module_id,
    module_items, resolve_alias, resolve_type_ref,
};
use pkl_vfs::StandardSourceLoader;
use std::sync::Arc;

fn database() -> RootDatabase {
    RootDatabase::new(
        Arc::new(StandardSourceLoader::default()),
        Default::default(),
    )
}
fn put(db: &mut RootDatabase, name: &str, text: &str) -> File {
    db.upsert_file(
        &Uri::parse(format!("file:///metadata/{name}.pkl")).unwrap(),
        Arc::from(text),
        None,
        FileStatus::Present,
    )
    .unwrap()
}
fn class(db: &RootDatabase, file: File, name: &str) -> DeclId {
    let index = module_items(db, file)
        .classes
        .iter()
        .find(|(_, class)| class.declaration.name.text(db) == name)
        .unwrap()
        .0;
    DeclId {
        file,
        root: file,
        item: ItemId::Class(index),
    }
}
fn named(db: &RootDatabase, name: &str) -> TypeRef {
    TypeRef::Named {
        path: vec![Name::new(db, name.to_owned())],
        arguments: vec![],
    }
}

#[test]
fn all_lowered_type_variants_resolve_without_source_locations() {
    let mut db = database();
    let file = put(&mut db, "variants", "class Owner<T> {}\n");
    let owner = class(&db, file, "Owner");
    let resolve = |ty: &TypeRef| resolve_type_ref(&db, owner, ty);
    let int = named(&db, "Int");
    let int_type = base_module(&db).int_type().clone();
    assert_eq!(resolve(&TypeRef::Unknown), Type::Unknown);
    assert_eq!(resolve(&TypeRef::Nothing), Type::Nothing);
    assert_eq!(
        resolve(&TypeRef::Module),
        base_module(&db).module_type().clone()
    );
    assert_eq!(
        resolve(&TypeRef::StringLiteral("hello".into())),
        Type::string_literal("hello")
    );
    assert_eq!(resolve(&int), int_type);
    assert_eq!(
        resolve(&TypeRef::Parenthesized(Box::new(int.clone()))),
        int_type
    );
    assert_eq!(resolve(&TypeRef::Default(Box::new(int.clone()))), int_type);
    assert_eq!(
        resolve(&TypeRef::Nullable(Box::new(int.clone()))),
        int_type.clone().nullable(&db)
    );
    assert_eq!(
        resolve(&TypeRef::Union(
            Box::new(int.clone()),
            Box::new(TypeRef::Nothing)
        )),
        int_type
    );
    let function = TypeRef::Function {
        parameters: vec![int.clone()],
        result: Box::new(named(&db, "String")),
    };
    assert_eq!(
        resolve(&function),
        base_module(&db)
            .function_n_type(1)
            .unwrap()
            .with_type_args(vec![
                int_type.clone(),
                base_module(&db).string_type().clone()
            ])
    );
    let constrained = resolve(&TypeRef::Constrained {
        base: Box::new(int),
        constraints: vec!["this > 0".into()],
    });
    assert_eq!(constrained.unconstrained(), &int_type);
    assert_eq!(constrained.constraints().len(), 1);
}

#[test]
fn generic_aliases_and_inherited_members_substitute_stable_parameters() {
    let mut db = database();
    let file = put(
        &mut db,
        "generic",
        "typealias Identity<T> = T\nopen class Parent<T> { value: T }\nclass Child<U> extends Parent<Identity<U>> {}\n",
    );
    let owner = class(&db, file, "Child");
    let int = base_module(&db).int_type().clone();
    let child = decl_type(&db, owner).with_type_args(vec![int.clone()]);
    assert_eq!(find_member(&db, &child, "value").unwrap().ty, int);
    let alias_index = module_items(&db, file)
        .type_aliases
        .iter()
        .next()
        .unwrap()
        .0;
    let alias = DeclId {
        file,
        root: file,
        item: ItemId::TypeAlias(alias_index),
    };
    assert_eq!(resolve_alias(&db, alias, std::slice::from_ref(&int)), int);
}

#[test]
fn method_type_parameters_do_not_shadow_qualified_imports() {
    let mut db = database();
    let imported = put(&mut db, "imported", "class Export {}\n");
    let file = put(
        &mut db,
        "shadow",
        "import \"imported.pkl\" as T\nclass Owner<T> { function f<T>(): T = throw(\"x\") }\n",
    );
    let method_index = module_items(&db, file).methods.iter().next().unwrap().0;
    let method = DeclId {
        file,
        root: file,
        item: ItemId::Method(method_index),
    };
    let qualified = TypeRef::Named {
        path: vec![Name::new(&db, "T"), Name::new(&db, "Export")],
        arguments: vec![],
    };
    assert_eq!(
        resolve_type_ref(&db, method, &qualified),
        decl_type(
            &db,
            DeclId {
                root: file,
                ..class(&db, imported, "Export")
            }
        )
    );
}

#[test]
fn sdk_ids_are_canonical_and_headers_survive_source_displacement() {
    let mut db = database();
    let a = put(&mut db, "a", "class A { value: Int = 1 }\n");
    let b = put(&mut db, "b", "class B {}\n");
    let sdk = db
        .file(&Uri::parse("pkl:base".to_owned()).unwrap())
        .unwrap();
    assert_eq!(module_id(&db, sdk, a), module_id(&db, sdk, b));
    let owner = class(&db, a, "A");
    let before = decl_type(&db, owner);
    let value = find_member(&db, &before, "value").unwrap().ty;
    put(
        &mut db,
        "a",
        "\n\n// displacement\nclass A { value: Int = 100 + 20 }\n",
    );
    assert_eq!(decl_type(&db, owner), before);
    assert_eq!(find_member(&db, &before, "value").unwrap().ty, value);
    let ItemId::Class(index) = owner.item else {
        unreachable!()
    };
    assert_eq!(
        before,
        Type::class(ClassId::new(&db, module_id(&db, a, a), index))
    );
}

#[test]
fn stale_metadata_ids_resolve_conservatively_after_deletion() {
    let mut db = database();
    let file = put(
        &mut db,
        "stale",
        "class Owner<T> { value: T }\ntypealias Alias<T> = T\n",
    );
    let owner = class(&db, file, "Owner");
    let alias = DeclId {
        file,
        root: file,
        item: ItemId::TypeAlias(
            module_items(&db, file)
                .type_aliases
                .iter()
                .next()
                .unwrap()
                .0,
        ),
    };
    let property = DeclId {
        file,
        root: file,
        item: ItemId::Property(module_items(&db, file).properties.iter().next().unwrap().0),
    };
    let old_class = decl_type(&db, owner);
    put(&mut db, "stale", "");
    assert_eq!(decl_type(&db, owner), Type::Unknown);
    assert_eq!(decl_type(&db, property), Type::Unknown);
    assert_eq!(resolve_alias(&db, alias, &[]), Type::Unknown);
    assert_eq!(
        resolve_type_ref(&db, owner, &named(&db, "T")),
        Type::Unknown
    );
    assert!(pkl_types::resolver::super_type(&old_class, &db).is_none());
}

#[test]
fn inferred_members_retain_receiver_substitutions_transitively() {
    let mut db = database();
    let file = put(
        &mut db,
        "bound",
        "class Box<T> { value: T; copy = value; another = copy; nested = let (first = copy) let (second = first) second; fromNested = nested }\n",
    );
    let owner = class(&db, file, "Box");
    for argument in [
        base_module(&db).int_type().clone(),
        base_module(&db).string_type().clone(),
    ] {
        let receiver = decl_type(&db, owner).with_type_args(vec![argument.clone()]);
        for name in ["copy", "another", "fromNested"] {
            assert_eq!(
                find_member(&db, &receiver, name).unwrap().ty,
                argument,
                "{name}"
            );
        }
    }
}

#[test]
fn own_imports_name_module_types_before_local_and_base_types() {
    // Mirrors pkl-core `ResolveSimpleDeclaredTypeNode`: the enclosing module's
    // imports (local members) are searched before its classes, aliases and pkl:base.
    let mut db = database();
    let imported = put(&mut db, "namespace_target", "class Export {}\n");
    let file = put(
        &mut db,
        "namespace_use",
        "import \"namespace_target.pkl\" as Shared\nimport \"namespace_target.pkl\" as Local\nimport \"pkl:json\" as String\nclass Shared {}\ntypealias Local = Int\nclass Owner {}\n",
    );
    let owner = class(&db, file, "Owner");
    let module_type = |target: File| Type::Module {
        module_id: module_id(&db, target, file),
    };
    let json = db
        .file(&Uri::parse("pkl:json".to_owned()).unwrap())
        .unwrap();
    assert_eq!(
        resolve_type_ref(&db, owner, &named(&db, "String")),
        module_type(json)
    );
    assert_eq!(
        resolve_type_ref(&db, owner, &named(&db, "Local")),
        module_type(imported)
    );
    assert_eq!(
        resolve_type_ref(&db, owner, &named(&db, "Shared")),
        module_type(imported)
    );
    let path = TypeRef::Named {
        path: vec![Name::new(&db, "Shared"), Name::new(&db, "Export")],
        arguments: vec![],
    };
    assert_eq!(
        resolve_type_ref(&db, owner, &path),
        decl_type(
            &db,
            DeclId {
                root: file,
                ..class(&db, imported, "Export")
            }
        )
    );
}

#[test]
fn missing_source_invalidates_headers_and_recovers_without_text_changes() {
    use salsa::Setter;
    let mut db = database();
    let target = put(
        &mut db,
        "availability_target",
        "class Export { value: Int = 1 }\nvalue: Int = 1\n",
    );
    let file = put(
        &mut db,
        "availability_use",
        "import \"availability_target.pkl\" as target\nvalue: target.Export\n",
    );
    let class_decl = class(&db, target, "Export");
    let property = DeclId {
        file: target,
        root: target,
        item: ItemId::Property(
            module_items(&db, target)
                .properties
                .iter()
                .next()
                .unwrap()
                .0,
        ),
    };
    let imported_use = DeclId {
        file,
        root: file,
        item: ItemId::Property(module_items(&db, file).properties.iter().next().unwrap().0),
    };
    let class_before = decl_type(&db, class_decl);
    let property_before = decl_type(&db, property);
    let imported_before = decl_type(&db, imported_use);
    assert_eq!(
        find_member(&db, &class_before, "value").unwrap().ty,
        property_before
    );
    let class_node = pkl_types::item_node(&db, class_decl).unwrap();
    target.set_status(&mut db).to(FileStatus::Missing);
    assert!(module_items(&db, target).declarations.is_empty());
    assert!(pkl_types::item_source_map(&db, target).nodes.is_empty());
    assert_eq!(decl_type(&db, class_decl), Type::Unknown);
    assert_eq!(
        pkl_types::resolver::definition_type(&db, class_node, target),
        Type::Unknown
    );
    assert!(find_member(&db, &class_before, "value").is_none());
    assert_eq!(decl_type(&db, property), Type::Unknown);
    assert_eq!(decl_type(&db, imported_use), Type::Unknown);
    target.set_status(&mut db).to(FileStatus::Present);
    assert_eq!(decl_type(&db, class_decl), class_before);
    assert_eq!(decl_type(&db, property), property_before);
    assert_eq!(decl_type(&db, imported_use), imported_before);
}
