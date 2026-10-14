//! Cache lookup tests; no real `pkl` CLI or ZIP extraction is needed.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use lspkl_project::cli::PklCli;
use lspkl_project::dto::{
    Checksums, DerivedProjectMetadata, LocalDependency, PackageDependency, PackageUri, PklProject,
    ProjectDeps, ResolvedDependency,
};
use lspkl_project::package_manager::PackageManager;
use lspkl_project::{
    import_resolver::ImportResolver,
    routing::{RoutingFacts, RoutingRequest},
};
use lspkl_vfs::{Source, StandardSourceLoader};
use std::cell::RefCell;

fn package_source(cache: &Path, package: &PackageDependency) -> Source {
    let zip = lspkl_vfs::path_to_uri(&cache.join(&package.uri.relative_zip_files()[0])).unwrap();
    Source {
        url: fluent_uri::Uri::parse(format!("jar:{zip}!/source.pkl")).unwrap(),
        text: "".into(),
    }
}

#[derive(Default)]
struct CapturedDependency(RefCell<Option<RoutingRequest>>);
impl RoutingFacts for CapturedDependency {
    fn root(&self, request: RoutingRequest) -> Option<fluent_uri::Uri<String>> {
        *self.0.borrow_mut() = Some(request);
        None
    }
}

// Exercise the production name lookup and dependency routing, not a second mapper.
fn dependencies(
    cache: &Path,
    package: &PackageDependency,
    context: Option<&PklProject>,
) -> BTreeMap<String, RoutingRequest> {
    let loader = StandardSourceLoader::new();
    let facts = CapturedDependency::default();
    let resolver = ImportResolver::new(&loader, cache).with_facts(&facts);
    let source = package_source(cache, package);
    resolver
        .dependency_names(&source, context)
        .into_iter()
        .map(|name| {
            resolver.completion_root(&format!("@{name}/"), &source, context);
            let dependency = facts
                .0
                .borrow_mut()
                .take()
                .expect("named dependency routing request");
            (name, dependency)
        })
        .collect()
}

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

fn assert_root_points_to_zip(roots: &lspkl_project::package_manager::PackageLibraryRoots) {
    let archive = roots
        .package_root
        .as_str()
        .strip_prefix("jar:")
        .unwrap()
        .strip_suffix("!/")
        .unwrap();
    let archive = fluent_uri::Uri::parse(archive.to_owned()).unwrap();
    let path = lspkl_vfs::url_to_path(&archive).unwrap();
    assert_eq!(
        path.canonicalize().unwrap(),
        roots.zip_file.canonicalize().unwrap()
    );
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
    assert_root_points_to_zip(&roots);
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
    assert_root_points_to_zip(&roots);
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
    assert!(dependencies(cache.path(), &dependency(), None).is_empty());
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
    let resolved = dependencies(cache.path(), &package, None);
    assert_eq!(
        resolved.keys().map(String::as_str).collect::<Vec<_>>(),
        ["lib", "tool"]
    );
    match &resolved["lib"] {
        RoutingRequest::Package {
            dependency: remote, ..
        } => {
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
    assert!(matches!(&resolved["tool"], RoutingRequest::Package { .. }));
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
    let resolved = dependencies(cache.path(), &package, Some(&context));
    match &resolved["lib"] {
        RoutingRequest::ModuleRoot(project_dir) => {
            assert_eq!(
                project_dir.canonicalize().unwrap(),
                workspace.path().join("local-lib").canonicalize().unwrap()
            );
        }
        other => panic!("expected local project, got {other:?}"),
    }
    assert!(
        matches!(&resolved["other"], RoutingRequest::Package { .. }),
        "unmatched dependencies fall back to remote when package has no owner"
    );
}

#[test]
fn package_context_preserves_unresolved_metadata_dependencies() {
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
            ResolvedDependency::Remote(lspkl_project::dto::RemoteDependency {
                uri: PackageUri::create("package://example.com/lib@1.0.0").unwrap(),
                checksums: None,
            }),
        )]),
    };
    let owner = project(workspace.path(), Some(deps));
    let resolved = dependencies(cache.path(), &package, Some(&owner));
    assert_eq!(
        resolved.keys().map(String::as_str).collect::<Vec<_>>(),
        ["lib", "unresolved"]
    );
    assert!(matches!(&resolved["lib"], RoutingRequest::Package { .. }));

    let owner_without_deps = project(workspace.path(), None);
    let without_deps = dependencies(cache.path(), &package, Some(&owner_without_deps));
    assert_eq!(
        without_deps.keys().map(String::as_str).collect::<Vec<_>>(),
        ["lib", "unresolved"]
    );
    assert!(matches!(
        &without_deps["lib"],
        RoutingRequest::Package { .. }
    ));
    assert!(matches!(
        &without_deps["unresolved"],
        RoutingRequest::Package { .. }
    ));
}

#[test]
fn invalid_cached_metadata_has_no_dependency_names() {
    let cache = tempfile::tempdir().unwrap();
    let package = dependency();
    write_candidate(cache.path(), &package.uri.relative_zip_files()[0]);
    let metadata_file = write_candidate(cache.path(), &package.uri.relative_metadata_files()[0]);
    fs::write(metadata_file, b"not json").unwrap();
    assert!(dependencies(cache.path(), &package, None).is_empty());
}
