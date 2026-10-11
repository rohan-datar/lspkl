//! Cross-file syntax-tree lookup through `AnalysisContext`.

mod harness;

use std::collections::HashMap;
use std::sync::Arc;

use pkl_syntax::ast::NodeKind;
use pkl_types::{AnalysisContext, FileId, NodeRef, Type};

#[test]
fn imported_declaration_can_be_read_through_its_node_ref() {
    let base = harness::load_test_base();
    let current = pkl_syntax::parse("import \"other.pkl\"\nlocal = 1\n").unwrap();
    let imported = Arc::new(pkl_syntax::parse("class Imported { value: String }\n").unwrap());
    let class = harness::find_first(&imported, |kind| matches!(kind, NodeKind::Class));
    let declaration = NodeRef::new(FileId(2), class);
    let trees = HashMap::from([(declaration.file_id, imported.clone())]);
    let ctx = AnalysisContext::new(&current, FileId(1), &base, &trees, harness::empty_imports());

    let target = ctx.tree_for(declaration.file_id).expect("imported tree");
    assert!(std::ptr::eq(target, imported.as_ref()));
    assert!(matches!(target.kind(declaration.node_id), NodeKind::Class));
    assert_eq!(target.name(declaration.node_id), Some("Imported"));
    assert_eq!(
        target.text(declaration.node_id),
        "class Imported { value: String }"
    );
}

#[test]
fn unknown_file_id_returns_none_with_a_populated_registry() {
    let base = harness::load_test_base();
    let current = pkl_syntax::parse("local = 1\n").unwrap();
    let imported = Arc::new(pkl_syntax::parse("imported = 2\n").unwrap());
    let trees = HashMap::from([(FileId(2), imported)]);
    let ctx = AnalysisContext::new(&current, FileId(1), &base, &trees, harness::empty_imports());

    assert!(ctx.tree_for(FileId(3)).is_none());
}

#[test]
fn current_and_base_trees_take_precedence_over_conflicting_registry_entries() {
    let base = harness::load_test_base();
    let current = pkl_syntax::parse("class Current {}\n").unwrap();
    let conflict = Arc::new(pkl_syntax::parse("class Wrong {}\n").unwrap());
    let trees = HashMap::from([(FileId(1), conflict.clone()), (FileId::BASE, conflict)]);
    let ctx = AnalysisContext::new(&current, FileId(1), &base, &trees, harness::empty_imports());

    let current_target = ctx.tree_for(FileId(1)).expect("current tree");
    assert!(std::ptr::eq(current_target, &current));
    let current_class = harness::find_first(current_target, |kind| matches!(kind, NodeKind::Class));
    assert_eq!(current_target.name(current_class), Some("Current"));

    let base_target = ctx.tree_for(FileId::BASE).expect("base tree");
    assert!(std::ptr::eq(base_target, base.tree().as_ref()));
    let Type::Class { class_node, .. } = base.string_type() else {
        panic!("String must be a class");
    };
    assert!(matches!(
        base_target.kind(class_node.node_id),
        NodeKind::Class
    ));
    assert_eq!(base_target.name(class_node.node_id), Some("String"));
}

#[test]
fn base_takes_precedence_when_current_file_id_is_base() {
    let base = harness::load_test_base();
    let current = pkl_syntax::parse("class Current {}\n").unwrap();
    let conflict = Arc::new(pkl_syntax::parse("class Wrong {}\n").unwrap());
    let trees = HashMap::from([(FileId::BASE, conflict)]);
    let ctx = AnalysisContext::new(
        &current,
        FileId::BASE,
        &base,
        &trees,
        harness::empty_imports(),
    );

    let target = ctx.tree_for(FileId::BASE).expect("base tree");
    assert!(std::ptr::eq(target, base.tree().as_ref()));
    let Type::Class { class_node, .. } = base.string_type() else {
        panic!("String must be a class");
    };
    assert_eq!(target.name(class_node.node_id), Some("String"));
}
