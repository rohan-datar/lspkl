//! Ordered directory-backed modulepath resolution tests.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use lspkl_project::dto::{DerivedProjectMetadata, EvaluatorSettings, PklProject};
use lspkl_project::modulepath::ModulepathResolver;
use lspkl_vfs::{SourceLoader, StandardSourceLoader};

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

#[test]
fn modulepaths_resolve_project_relative_entries_in_declared_order() {
    let temp = tempfile::tempdir().unwrap();
    let context = project(temp.path(), &["first", "second"]);
    let manager = StandardSourceLoader::new();
    let resolver = ModulepathResolver::new(&manager, temp.path());
    assert_eq!(
        resolver.module_paths(Some(&context)),
        [temp.path().join("first"), temp.path().join("second")]
    );
    assert!(resolver.module_paths(None).is_empty());
    let dotted = project(temp.path(), &["missing/../second"]);
    assert_eq!(
        resolver.module_paths(Some(&dotted)),
        [temp.path().join("second")]
    );
}

#[test]
fn absolute_lookup_uses_first_existing_regular_file() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    fs::write(first.join("same.pkl"), "first disk").unwrap();
    fs::write(second.join("same.pkl"), "second disk").unwrap();
    fs::write(second.join("only.pkl"), "second only").unwrap();
    fs::create_dir(first.join("folder.pkl")).unwrap();
    let context = project(temp.path(), &["first", "second"]);
    let manager = StandardSourceLoader::new();
    let resolver = ModulepathResolver::new(&manager, temp.path());
    let chosen = resolver
        .resolve_absolute("/same.pkl", Some(&context))
        .unwrap();
    assert_eq!(chosen.contents(), "first disk");
    assert_eq!(
        resolver
            .resolve_absolute("/only.pkl", Some(&context))
            .unwrap()
            .contents(),
        "second only"
    );
    assert!(
        resolver
            .resolve_absolute("/folder.pkl", Some(&context))
            .is_none()
    );
    assert!(
        resolver
            .resolve_absolute("/missing.pkl", Some(&context))
            .is_none()
    );
    assert!(resolver.resolve_absolute("/same.pkl", None).is_none());
}

#[test]
fn relative_lookup_merges_roots_and_path_membership_is_component_safe() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("root");
    let second = temp.path().join("second");
    let prefix_trap = temp.path().join("root2");
    // The first root need not have the importing file's parent directory.
    // `nested/../shared.pkl` must still find its root-level shared file.
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(second.join("nested")).unwrap();
    fs::create_dir_all(&prefix_trap).unwrap();
    fs::write(second.join("nested/source.pkl"), "source").unwrap();
    fs::write(first.join("shared.pkl"), "first shared").unwrap();
    fs::write(second.join("shared.pkl"), "second shared").unwrap();
    let context = project(temp.path(), &["root", "second"]);
    let manager = StandardSourceLoader::new();
    let source_url = fluent_uri::Uri::parse(
        url::Url::from_file_path(second.join("nested/source.pkl"))
            .unwrap()
            .to_string(),
    )
    .unwrap();
    let source = manager.load(&source_url, temp.path()).unwrap();
    let resolver = ModulepathResolver::new(&manager, temp.path());
    assert_eq!(
        resolver
            .resolve_relative(&source, "../shared.pkl", Some(&context))
            .unwrap()
            .contents(),
        "first shared"
    );
    assert_eq!(
        resolver
            .resolve_relative(&source, "/shared.pkl", Some(&context))
            .unwrap()
            .contents(),
        "first shared"
    );
    assert!(
        resolver
            .resolve_relative(&source, "missing.pkl", Some(&context))
            .is_none()
    );
}
