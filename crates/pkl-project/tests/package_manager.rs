//! Cache lookup tests; no real `pkl` CLI or ZIP extraction is needed.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use pkl_project::cli::PklCli;
use pkl_project::dto::{
    Checksums, Dependency, DerivedProjectMetadata, LocalDependency, PackageDependency, PackageUri,
    PklProject, ProjectDeps, ResolvedDependency,
};
use pkl_project::package_manager::PackageManager;

fn dependency() -> PackageDependency {
    PackageDependency {
        uri: PackageUri::create("package://example.com/bar/foo@1.2.3").unwrap(),
        checksums: None,
    }
}

fn write_candidate(cache: &Path, relative: &str) -> PathBuf {
    let path = cache.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"fixture").unwrap();
    path
}

#[test]
fn missing_metadata_or_zip_returns_none() {
    let cache = tempfile::tempdir().unwrap();
    let dependency = dependency();
    let manager = PackageManager::new(PklCli::default());
    assert!(
        manager
            .get_library_roots(&dependency, cache.path())
            .unwrap()
            .is_none()
    );

    write_candidate(cache.path(), &dependency.uri.relative_metadata_files()[0]);
    assert!(
        manager
            .get_library_roots(&dependency, cache.path())
            .unwrap()
            .is_none()
    );

    fs::remove_file(
        cache
            .path()
            .join(&dependency.uri.relative_metadata_files()[0]),
    )
    .unwrap();
    write_candidate(cache.path(), &dependency.uri.relative_zip_files()[0]);
    assert!(
        manager
            .get_library_roots(&dependency, cache.path())
            .unwrap()
            .is_none()
    );
}

#[test]
fn roots_prefer_package_2_when_both_versions_exist() {
    let cache = tempfile::tempdir().unwrap();
    let dependency = dependency();
    let metadata = dependency.uri.relative_metadata_files();
    let zips = dependency.uri.relative_zip_files();
    for candidate in metadata.iter().chain(zips.iter()) {
        write_candidate(cache.path(), candidate);
    }
    let manager = PackageManager::new(PklCli::default());
    let roots = manager
        .get_library_roots(&dependency, cache.path())
        .unwrap()
        .unwrap();
    assert_eq!(roots.metadata_file, cache.path().join(&metadata[0]));
    assert_eq!(roots.zip_file, cache.path().join(&zips[0]));
    assert_eq!(
        roots.package_root.as_str(),
        format!(
            "jar:{}!/",
            url::Url::from_file_path(&roots.zip_file).unwrap()
        )
    );
}

#[test]
fn metadata_and_zip_choose_cache_versions_independently() {
    let cache = tempfile::tempdir().unwrap();
    let dependency = dependency();
    let metadata = dependency.uri.relative_metadata_files();
    let zips = dependency.uri.relative_zip_files();
    let metadata_file = write_candidate(cache.path(), &metadata[0]);
    let zip_file = write_candidate(cache.path(), &zips[1]);

    let manager = PackageManager::new(PklCli::default());
    let roots = manager
        .get_library_roots(&dependency, cache.path())
        .unwrap()
        .unwrap();
    assert_eq!(roots.metadata_file, metadata_file);
    assert_eq!(roots.zip_file, zip_file);
    assert_eq!(
        roots.package_root.as_str(),
        format!("jar:{}!/", url::Url::from_file_path(&zip_file).unwrap())
    );
}

fn write_metadata(cache: &Path, dependency: &PackageDependency, dependencies: serde_json::Value) {
    let metadata_file = write_candidate(cache, &dependency.uri.relative_metadata_files()[0]);
    write_candidate(cache, &dependency.uri.relative_zip_files()[0]);
    let metadata = serde_json::json!({
        "name": "test-package",
        "packageZipUrl": "https://example.com/test-package.zip",
        "version": "1.2.3",
        "packageUri": "package://example.com/bar/foo@1.2.3",
        "dependencies": dependencies,
        "futureField": true
    });
    fs::write(metadata_file, serde_json::to_vec(&metadata).unwrap()).unwrap();
}

fn project(dir: &Path, deps: Option<ProjectDeps>) -> PklProject {
    PklProject {
        metadata: DerivedProjectMetadata {
            project_file_uri: url::Url::from_file_path(dir.join("PklProject"))
                .unwrap()
                .to_string(),
            package_uri: None,
            declared_dependencies: BTreeMap::new(),
            evaluator_settings: None,
        },
        project_deps: deps,
    }
}

#[test]
fn dependencies_are_absent_when_package_cache_is_missing() {
    let cache = tempfile::tempdir().unwrap();
    let manager = PackageManager::new(PklCli::default());
    assert!(
        manager
            .get_resolved_dependencies(&dependency(), None, None, cache.path())
            .unwrap()
            .is_none()
    );
}

#[test]
fn metadata_dependencies_keep_names_and_use_remote_fallback_without_context() {
    let cache = tempfile::tempdir().unwrap();
    let package = dependency();
    write_metadata(
        cache.path(),
        &package,
        serde_json::json!({
            "lib": {
                "uri": "package://example.com/lib@1.0.0",
                "checksums": { "sha256": "cafebabe" }
            },
            "tool": {
                "uri": "package://example.com/tool@2.0.0",
                "checksums": null
            }
        }),
    );
    let manager = PackageManager::new(PklCli::default());
    let resolved = manager
        .get_resolved_dependencies(&package, None, None, cache.path())
        .unwrap()
        .expect("cached metadata");
    assert_eq!(
        resolved.keys().map(String::as_str).collect::<Vec<_>>(),
        ["lib", "tool"]
    );
    match &resolved["lib"] {
        Dependency::Package(remote) => {
            assert_eq!(
                remote.uri.to_display_string(),
                "package://example.com/lib@1.0.0"
            );
            assert_eq!(
                remote.checksums,
                Some(Checksums {
                    sha256: "cafebabe".into()
                })
            );
        }
        other => panic!("expected remote dependency, got {other:?}"),
    }
    assert!(matches!(&resolved["tool"], Dependency::Package(_)));
}

#[test]
fn context_can_resolve_a_package_dependency_to_a_local_project() {
    let cache = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let owner_dir = workspace.path().join("owner");
    fs::create_dir(&owner_dir).unwrap();
    fs::create_dir(workspace.path().join("local-lib")).unwrap();
    let package = dependency();
    write_metadata(
        cache.path(),
        &package,
        serde_json::json!({
            "lib": { "uri": "package://example.com/lib@1.0.0", "checksums": null },
            "other": { "uri": "package://example.com/other@2.0.0", "checksums": null }
        }),
    );
    let deps = ProjectDeps {
        schema_version: 1,
        resolved_dependencies: BTreeMap::from([(
            "package://example.com/lib@1".into(),
            ResolvedDependency::Local(LocalDependency {
                uri: PackageUri::create("package://example.com/lib@1.0.0").unwrap(),
                path: "../local-lib".into(),
            }),
        )]),
    };
    let context = project(&owner_dir, Some(deps));
    let manager = PackageManager::new(PklCli::default());
    let resolved = manager
        .get_resolved_dependencies(&package, None, Some(&context), cache.path())
        .unwrap()
        .expect("cached metadata");
    match &resolved["lib"] {
        Dependency::LocalProject(local) => {
            assert_eq!(local.project_dir, owner_dir.join("../local-lib"));
        }
        other => panic!("expected local project, got {other:?}"),
    }
    assert!(
        matches!(&resolved["other"], Dependency::Package(_)),
        "unmatched dependencies fall back to remote when package has no owner"
    );
}

#[test]
fn owning_project_keeps_only_resolved_dependencies() {
    let cache = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let package = dependency();
    write_metadata(
        cache.path(),
        &package,
        serde_json::json!({
            "lib": { "uri": "package://example.com/lib@1.0.0", "checksums": null },
            "unresolved": { "uri": "package://example.com/missing@2.0.0", "checksums": null }
        }),
    );
    let deps = ProjectDeps {
        schema_version: 1,
        resolved_dependencies: BTreeMap::from([(
            "package://example.com/lib@1".into(),
            ResolvedDependency::Remote(pkl_project::dto::RemoteDependency {
                uri: PackageUri::create("package://example.com/lib@1.0.0").unwrap(),
                checksums: None,
            }),
        )]),
    };
    let owner = project(workspace.path(), Some(deps));
    let manager = PackageManager::new(PklCli::default());
    let resolved = manager
        .get_resolved_dependencies(&package, Some(&owner), None, cache.path())
        .unwrap()
        .expect("owner has project deps");
    assert_eq!(
        resolved.keys().map(String::as_str).collect::<Vec<_>>(),
        ["lib"]
    );
    assert!(matches!(&resolved["lib"], Dependency::Package(_)));

    let owner_without_deps = project(workspace.path(), None);
    assert!(
        manager
            .get_resolved_dependencies(&package, Some(&owner_without_deps), None, cache.path())
            .unwrap()
            .is_none(),
        "owner without project deps cannot resolve package metadata"
    );
}

#[test]
fn invalid_cached_metadata_is_a_parse_error() {
    let cache = tempfile::tempdir().unwrap();
    let package = dependency();
    write_candidate(cache.path(), &package.uri.relative_zip_files()[0]);
    let metadata_file = write_candidate(cache.path(), &package.uri.relative_metadata_files()[0]);
    fs::write(metadata_file, b"not json").unwrap();
    let manager = PackageManager::new(PklCli::default());
    assert!(
        manager
            .get_resolved_dependencies(&package, None, None, cache.path())
            .is_err()
    );
}
