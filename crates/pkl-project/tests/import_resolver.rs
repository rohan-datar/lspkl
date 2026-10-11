//! Hermetic module URI routing tests.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use pkl_project::dto::{DerivedProjectMetadata, EvaluatorSettings, PklProject};
use pkl_project::import_resolver::ImportResolver;
use pkl_vfs::FileManager;

fn project(dir: &Path, roots: &[&str]) -> PklProject {
    PklProject {
        metadata: DerivedProjectMetadata {
            project_file_uri: url::Url::from_file_path(dir.join("PklProject"))
                .unwrap()
                .to_string(),
            package_uri: None,
            declared_dependencies: BTreeMap::new(),
            evaluator_settings: Some(EvaluatorSettings {
                module_cache_dir: None,
                module_path: Some(roots.iter().map(|root| (*root).to_owned()).collect()),
            }),
        },
        project_deps: None,
    }
}

fn file_uri(path: &Path) -> fluent_uri::Uri<String> {
    fluent_uri::Uri::parse(url::Url::from_file_path(path).unwrap().to_string()).unwrap()
}

#[test]
fn modulepath_import_uses_project_roots_and_reuses_editor_buffer() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    fs::write(
        temp.path().join("source.pkl"),
        "import \"modulepath:/lib.pkl\"
",
    )
    .unwrap();
    fs::write(first.join("lib.pkl"), "disk-first").unwrap();
    fs::write(second.join("lib.pkl"), "disk-second").unwrap();
    fs::write(second.join("only.pkl"), "second-only").unwrap();
    let ctx = project(temp.path(), &["first", "second"]);
    let files = FileManager::new();
    let source = files
        .get_or_load(&file_uri(&temp.path().join("source.pkl")))
        .unwrap();
    let opened = files.open(
        file_uri(&first.join("lib.pkl")),
        "editor-first".into(),
        Some(1),
    );
    let resolver = ImportResolver::new(&files);

    let selected = resolver
        .resolve_file("modulepath:/lib.pkl", &source, Some(&ctx))
        .unwrap();
    assert!(Arc::ptr_eq(&selected, &opened));
    assert_eq!(selected.contents(), "editor-first");
    assert_eq!(
        resolver
            .resolve_file("modulepath:/only.pkl", &source, Some(&ctx))
            .unwrap()
            .contents(),
        "second-only"
    );
    assert!(
        resolver
            .resolve_file("modulepath:/lib.pkl", &source, None)
            .is_none()
    );
}

#[test]
fn relative_filesystem_import_loads_sibling_and_reuses_editor_buffer() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("src");
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("source.pkl"),
        "import \"./sibling.pkl\"
",
    )
    .unwrap();
    fs::write(dir.join("sibling.pkl"), "disk").unwrap();
    let files = FileManager::new();
    let source = files
        .get_or_load(&file_uri(&dir.join("source.pkl")))
        .unwrap();
    let opened = files.open(file_uri(&dir.join("sibling.pkl")), "editor".into(), Some(2));
    let resolver = ImportResolver::new(&files);

    let selected = resolver
        .resolve_file("./sibling.pkl", &source, None)
        .unwrap();
    assert!(Arc::ptr_eq(&selected, &opened));
    assert!(
        resolver
            .resolve_file("./missing.pkl", &source, None)
            .is_none()
    );
}

#[test]
fn relative_import_in_modulepath_file_uses_ordered_project_roots() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    // File URIs escape this space; membership must use the decoded path.
    let second = temp.path().join("second root");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(second.join("nested")).unwrap();
    fs::write(
        second.join("nested/source.pkl"),
        "import \"../shared.pkl\"
",
    )
    .unwrap();
    fs::write(first.join("shared.pkl"), "first").unwrap();
    fs::write(second.join("shared.pkl"), "second").unwrap();
    let ctx = project(temp.path(), &["first", "second root"]);
    let files = FileManager::new();
    let source = files
        .get_or_load(&file_uri(&second.join("nested/source.pkl")))
        .unwrap();
    let resolver = ImportResolver::new(&files);

    assert_eq!(
        resolver
            .resolve_file("../shared.pkl", &source, Some(&ctx))
            .unwrap()
            .contents(),
        "first"
    );
    assert_eq!(
        resolver
            .resolve_file("../shared.pkl", &source, None)
            .unwrap()
            .contents(),
        "second"
    );
}

#[test]
fn unsupported_or_missing_imports_return_none() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("source.pkl");
    fs::write(
        &path,
        "import \"missing.pkl\"
",
    )
    .unwrap();
    let files = FileManager::new();
    let source = files.get_or_load(&file_uri(&path)).unwrap();
    let ctx = project(temp.path(), &["missing-root"]);
    let resolver = ImportResolver::new(&files);

    assert!(
        resolver
            .resolve_file("unsupported://example.com/test.pkl", &source, Some(&ctx))
            .is_none()
    );
    assert!(
        resolver
            .resolve_file("modulepath:/missing.pkl", &source, Some(&ctx))
            .is_none()
    );
    assert!(
        resolver
            .resolve_file("missing.pkl", &source, None)
            .is_none()
    );
}
