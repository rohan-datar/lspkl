use fluent_uri::Uri;
use pkl_project::dto::*;
use pkl_project::import_resolver::ImportResolver;
use pkl_vfs::FileManager;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn uri(path: &Path) -> Uri<String> {
    Uri::parse(url::Url::from_file_path(path).unwrap().to_string()).unwrap()
}
fn archive(path: &Path, entries: &[(&str, &str)]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, text) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    writer.finish().unwrap();
}
fn package(
    cache: &Path,
    name: &str,
    dependencies: BTreeMap<String, RemoteDependency>,
    entries: &[(&str, &str)],
) -> PackageUri {
    let package = PackageUri::create(&format!("package://example.test/{name}@1.0.0")).unwrap();
    let metadata = PackageMetadata {
        name: name.into(),
        version: "1.0.0".into(),
        package_zip_url: "https://example.test/package.zip".into(),
        package_uri: package.clone(),
        dependencies,
    };
    let json = cache.join(&package.relative_metadata_files()[0]);
    fs::create_dir_all(json.parent().unwrap()).unwrap();
    fs::write(json, serde_json::to_vec(&metadata).unwrap()).unwrap();
    archive(&cache.join(&package.relative_zip_files()[0]), entries);
    package
}
fn project(dir: &Path, roots: &[&str]) -> PklProject {
    PklProject {
        metadata: DerivedProjectMetadata {
            project_file_uri: uri(&dir.join("PklProject")).to_string(),
            package_uri: None,
            declared_dependencies: BTreeMap::new(),
            evaluator_settings: Some(EvaluatorSettings {
                module_cache_dir: None,
                module_path: Some(roots.iter().map(|s| (*s).into()).collect()),
            }),
        },
        project_deps: Some(ProjectDeps {
            schema_version: 1,
            resolved_dependencies: BTreeMap::new(),
        }),
    }
}
fn contents(files: Vec<std::sync::Arc<pkl_vfs::VirtualFile>>) -> Vec<String> {
    files.iter().map(|f| f.contents()).collect()
}

#[test]
fn missing_package_download_targets_distinguish_uncached_packages_from_bad_members() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let files = FileManager::new();
    let source = files.open(uri(&temp.path().join("main.pkl")), String::new(), None);
    let resolver = ImportResolver::with_settings(&files, &[], &cache);
    let missing = PackageUri::create("package://example.test/missing@1.0.0").unwrap();
    let reference = format!("{missing}#/main.pkl");
    assert_eq!(
        resolver.missing_package_uri(&reference, &source, None),
        Some(missing.to_string())
    );
    assert!(
        resolver
            .missing_package_uri("@unknown/main.pkl", &source, None)
            .is_none()
    );
    let mut context = project(temp.path(), &[]);
    context
        .metadata
        .declared_dependencies
        .insert("known".into(), missing.clone());
    assert_eq!(
        resolver.missing_package_uri("@known/main.pkl", &source, Some(&context)),
        Some(missing.to_string())
    );
    assert_eq!(
        resolver.dependency_names(&source, Some(&context)),
        ["known"]
    );
    let cached = package(
        &cache,
        "cached",
        BTreeMap::from([(
            "transitive".into(),
            RemoteDependency {
                uri: missing.clone(),
                checksums: None,
            },
        )]),
        &[("main.pkl", "value = 1")],
    );
    let cached_source = resolver
        .resolve_file(&format!("{cached}#/main.pkl"), &source, None)
        .unwrap();
    assert!(
        resolver
            .resolve_file(&format!("{cached}#/bad.pkl"), &source, None)
            .is_none()
    );
    assert!(
        resolver
            .missing_package_uri(&format!("{cached}#/bad.pkl"), &source, None)
            .is_none()
    );
    assert_eq!(
        resolver.missing_package_uri("@transitive/main.pkl", &cached_source, None),
        Some(missing.to_string())
    );
    assert_eq!(
        resolver.dependency_names(&cached_source, None),
        ["transitive"]
    );
    assert!(
        resolver
            .missing_package_uri("@unknown/main.pkl", &cached_source, Some(&context))
            .is_none()
    );
    context
        .metadata
        .declared_dependencies
        .insert("cached".into(), cached);
    assert!(
        resolver
            .missing_package_uri("@cached/bad.pkl", &source, Some(&context))
            .is_none()
    );
    let pinned = "package://example.test/pinned@1.0.0::sha256:abc";
    assert_eq!(
        resolver.missing_package_uri(&format!("{pinned}#/main.pkl"), &source, None),
        Some(pinned.into())
    );
}

#[test]
fn cached_packages_route_transitive_names_and_context_local_overrides() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let b = package(
        &cache,
        "b",
        BTreeMap::new(),
        &[("lib.pkl", "b"), ("a.pkl", "a")],
    );
    let a = package(
        &cache,
        "a",
        BTreeMap::from([(
            "transitive".into(),
            RemoteDependency {
                uri: b.clone(),
                checksums: None,
            },
        )]),
        &[("nested/source.pkl", "source"), ("sibling.pkl", "sibling")],
    );
    fs::write(temp.path().join("source.pkl"), "source").unwrap();
    let files = FileManager::new();
    let source = files
        .get_or_load(&uri(&temp.path().join("source.pkl")))
        .unwrap();
    let resolver = ImportResolver::with_settings(&files, &[], &cache);
    let from_a = resolver
        .resolve_file(&format!("{a}#/nested/source.pkl"), &source, None)
        .unwrap();
    assert_eq!(
        resolver
            .resolve_file("../sibling.pkl", &from_a, None)
            .unwrap()
            .contents(),
        "sibling"
    );
    assert_eq!(
        resolver
            .resolve_file("@transitive/lib.pkl", &from_a, None)
            .unwrap()
            .contents(),
        "b"
    );
    assert_eq!(
        contents(resolver.resolve_glob("@transitive/*.pkl", &from_a, None)),
        ["a", "b"]
    );
    assert_eq!(
        contents(resolver.resolve_glob(&format!("{b}#/[al]*.pkl"), &source, None)),
        ["a", "b"]
    );
    let local = temp.path().join("local");
    fs::create_dir(&local).unwrap();
    fs::write(local.join("lib.pkl"), "override").unwrap();
    let mut context = project(temp.path(), &[]);
    // This project has no declaration named transitive: the source package owns that name.
    context
        .project_deps
        .as_mut()
        .unwrap()
        .resolved_dependencies
        .insert(
            b.to_display_string(),
            ResolvedDependency::Local(LocalDependency {
                uri: b,
                path: "local".into(),
            }),
        );
    assert!(
        resolver
            .missing_package_uri("@transitive/missing.pkl", &from_a, Some(&context))
            .is_none()
    );
    assert_eq!(
        resolver
            .resolve_file("@transitive/lib.pkl", &from_a, Some(&context))
            .unwrap()
            .contents(),
        "override"
    );
    assert!(
        resolver
            .resolve_file(
                uri(&local.join("lib.pkl")).as_str(),
                &from_a,
                Some(&context)
            )
            .is_none()
    );
    assert!(
        resolver
            .resolve_file("../../../source.pkl", &from_a, None)
            .is_none()
    );
    assert!(
        resolver
            .resolve_glob("../../../*.pkl", &from_a, None)
            .is_empty()
    );
    assert!(
        resolver
            .resolve_file(&format!("{a}#nested/source.pkl"), &source, None)
            .is_none()
    );
}

#[test]
fn project_named_remote_and_local_dependencies() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let remote = package(&cache, "remote", BTreeMap::new(), &[("lib.pkl", "remote")]);
    let local_uri = PackageUri::create("package://example.test/local@1.0.0").unwrap();
    fs::create_dir(temp.path().join("local")).unwrap();
    fs::write(temp.path().join("local/lib.pkl"), "local").unwrap();
    let mut context = project(temp.path(), &[]);
    context.metadata.declared_dependencies = BTreeMap::from([
        ("remote".into(), remote.clone()),
        ("local".into(), local_uri.clone()),
    ]);
    context.project_deps.as_mut().unwrap().resolved_dependencies = BTreeMap::from([
        (
            remote.to_display_string(),
            ResolvedDependency::Remote(RemoteDependency {
                uri: remote,
                checksums: None,
            }),
        ),
        (
            local_uri.to_display_string(),
            ResolvedDependency::Local(LocalDependency {
                uri: local_uri,
                path: "local".into(),
            }),
        ),
    ]);
    fs::write(temp.path().join("source.pkl"), "source").unwrap();
    let files = FileManager::new();
    let source = files
        .get_or_load(&uri(&temp.path().join("source.pkl")))
        .unwrap();
    let resolver = ImportResolver::with_settings(&files, &[], &cache);
    assert_eq!(
        resolver
            .resolve_file("@remote/lib.pkl", &source, Some(&context))
            .unwrap()
            .contents(),
        "remote"
    );
    assert_eq!(
        resolver
            .resolve_file("@local/lib.pkl", &source, Some(&context))
            .unwrap()
            .contents(),
        "local"
    );
    assert_eq!(
        contents(resolver.resolve_glob("@local/*.pkl", &source, Some(&context))),
        ["local"]
    );
    assert!(
        resolver
            .resolve_file("@missing/lib.pkl", &source, Some(&context))
            .is_none()
    );
}

#[test]
fn archive_and_global_modulepaths_shadow_and_glob_in_logical_order() {
    let temp = tempfile::tempdir().unwrap();
    archive(
        &temp.path().join("project.zip"),
        &[
            ("shared.pkl", "project"),
            ("nested/source.pkl", "source"),
            ("nested/z.pkl", "z"),
        ],
    );
    archive(
        &temp.path().join("global.jar"),
        &[
            ("shared.pkl", "global"),
            ("nested/a.pkl", "a"),
            ("global.pkl", "global-only"),
        ],
    );
    let context = project(temp.path(), &["project.zip"]);
    let globals: Vec<PathBuf> = vec![temp.path().join("global.jar")];
    let files = FileManager::new();
    let source = files.open(
        uri(&temp.path().join("source.pkl")),
        "source".into(),
        Some(1),
    );
    let resolver = ImportResolver::with_settings(&files, &globals, temp.path());
    let archived = resolver
        .resolve_file("modulepath:/nested/source.pkl", &source, Some(&context))
        .unwrap();
    assert_eq!(
        resolver
            .resolve_file("../shared.pkl", &archived, Some(&context))
            .unwrap()
            .contents(),
        "project"
    );
    assert_eq!(
        resolver
            .resolve_file("modulepath:/shared.pkl", &source, None)
            .unwrap()
            .contents(),
        "global"
    );
    assert_eq!(
        contents(resolver.resolve_glob("modulepath:/*.pkl", &source, Some(&context))),
        ["global-only", "project"]
    );
    assert_eq!(
        contents(resolver.resolve_glob("[az].pkl", &archived, Some(&context))),
        ["a", "z"]
    );
    assert!(
        resolver
            .resolve_file("modulepath:/../source.pkl", &source, Some(&context))
            .is_none()
    );
    assert!(
        resolver
            .resolve_glob("modulepath:/../*.pkl", &source, Some(&context))
            .is_empty()
    );
}

#[test]
fn filesystem_globs_upward_search_and_delegated_schemes() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("a/b")).unwrap();
    fs::write(temp.path().join("parent.pkl"), "parent").unwrap();
    fs::write(temp.path().join("a/b/source.pkl"), "source").unwrap();
    fs::write(temp.path().join("a/b/a.pkl"), "a").unwrap();
    fs::write(temp.path().join("a/b/z.pkl"), "z").unwrap();
    let files = FileManager::new();
    let source = files
        .get_or_load(&uri(&temp.path().join("a/b/source.pkl")))
        .unwrap();
    let https = files.open(
        Uri::parse("https://example.test/dir/source.pkl".to_owned()).unwrap(),
        "https".into(),
        None,
    );
    files.open(
        Uri::parse("https://example.test/lib.pkl".to_owned()).unwrap(),
        "https-lib".into(),
        None,
    );
    let resolver = ImportResolver::new(&files);
    assert_eq!(
        resolver
            .resolve_file(".../parent.pkl", &source, None)
            .unwrap()
            .contents(),
        "parent"
    );
    assert_eq!(
        contents(resolver.resolve_glob("{a,z}.pkl", &source, None)),
        ["a", "z"]
    );
    assert_eq!(
        contents(resolver.resolve_glob("../b/[az].pkl", &source, None)),
        ["a", "z"]
    );
    assert_eq!(
        resolver
            .resolve_file("../lib.pkl", &https, None)
            .unwrap()
            .contents(),
        "https-lib"
    );
    assert!(
        resolver
            .resolve_file(uri(&temp.path().join("parent.pkl")).as_str(), &https, None)
            .is_none()
    );
    assert!(resolver.resolve_glob("*.pkl", &https, None).is_empty());
    assert_eq!(
        resolver
            .resolve_file("pkl:base", &source, None)
            .unwrap()
            .url
            .as_str(),
        "pkl:base"
    );
    assert_eq!(
        resolver
            .resolve_file(uri(&temp.path().join("parent.pkl")).as_str(), &source, None)
            .unwrap()
            .contents(),
        "parent"
    );
}

#[test]
fn member_routes_reject_absolute_uri_injection() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let dep = package(
        &cache,
        "dep",
        BTreeMap::new(),
        &[("main.pkl", "module dep")],
    );
    let mut context = project(temp.path(), &["."]);
    context
        .metadata
        .declared_dependencies
        .insert("dep".into(), dep.clone());
    context
        .project_deps
        .as_mut()
        .unwrap()
        .resolved_dependencies
        .insert(
            dep.to_display_string(),
            ResolvedDependency::Remote(RemoteDependency {
                uri: dep.clone(),
                checksums: None,
            }),
        );
    let files = FileManager::new();
    let source = files.open(uri(&temp.path().join("source.pkl")), "source".into(), None);
    let private = temp.path().join("private.pkl");
    fs::write(&private, "private").unwrap();
    let resolver = ImportResolver::with_settings(&files, &[], &cache);
    let archived = resolver
        .resolve_file(&format!("{dep}#/main.pkl"), &source, Some(&context))
        .unwrap();
    let target = uri(&private);
    for reference in [
        format!("{dep}#/{target}"),
        format!("@dep/{target}"),
        format!("modulepath:/{target}"),
        format!("{dep}#/file%3A///{}", private.display()),
        format!("@dep/file%3A///{}", private.display()),
        format!("modulepath:/file%3A///{}", private.display()),
    ] {
        assert!(
            resolver
                .resolve_file(&reference, &archived, Some(&context))
                .is_none(),
            "injection resolved: {reference}"
        );
    }
    assert!(
        resolver
            .resolve_file(&format!(".../{target}"), &archived, Some(&context))
            .is_none()
    );
    assert!(
        resolver
            .resolve_file(
                &format!("{dep}#/%2F%2Fexample.test/private.pkl"),
                &source,
                Some(&context)
            )
            .is_none()
    );
    assert_eq!(
        resolver
            .resolve_file(target.as_str(), &source, Some(&context))
            .unwrap()
            .contents(),
        "private"
    );
}

#[test]
fn encoded_package_named_and_modulepath_globs_match_decoded_members() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let dep = package(
        &cache,
        "dep",
        BTreeMap::new(),
        &[("space dir/a.pkl", "a"), ("space dir/z.pkl", "z")],
    );
    let mut context = project(temp.path(), &["modules.jar"]);
    archive(
        &temp.path().join("modules.jar"),
        &[("space dir/a.pkl", "a"), ("space dir/z.pkl", "z")],
    );
    context
        .metadata
        .declared_dependencies
        .insert("dep".into(), dep.clone());
    context
        .project_deps
        .as_mut()
        .unwrap()
        .resolved_dependencies
        .insert(
            dep.to_display_string(),
            ResolvedDependency::Remote(RemoteDependency {
                uri: dep.clone(),
                checksums: None,
            }),
        );
    let files = FileManager::new();
    let source = files.open(uri(&temp.path().join("source.pkl")), "source".into(), None);
    let resolver = ImportResolver::with_settings(&files, &[], &cache);
    for reference in [
        format!("{dep}#/space%20dir"),
        "@dep/space%20dir".into(),
        "modulepath:/space%20dir".into(),
    ] {
        assert_eq!(
            resolver
                .resolve_file(&format!("{reference}/a.pkl"), &source, Some(&context))
                .unwrap()
                .contents(),
            "a"
        );
        assert_eq!(
            contents(resolver.resolve_glob(&format!("{reference}/*.pkl"), &source, Some(&context))),
            ["a", "z"],
            "glob: {reference}"
        );
    }
}
