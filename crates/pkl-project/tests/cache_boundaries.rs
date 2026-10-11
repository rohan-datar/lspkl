use pkl_project::{
    cli::PklCli,
    dto::{PackageDependency, PackageUri},
    package_manager::PackageManager,
};

#[test]
fn package_identities_reject_foreign_schemes_and_parent_segments() {
    for value in [
        "file://example.test/a@1.0.0",
        "https://example.test/a@1.0.0",
        "package://example.test/../outside@1.0.0",
        "package://example.test/a/../../outside@1.0.0",
    ] {
        assert!(PackageUri::create(value).is_err(), "{value}");
    }
    assert!(PackageUri::create("package://example.test/nested/a@1.0.0").is_ok());
}

#[test]
fn manually_constructed_dependencies_cannot_escape_the_cache() {
    let temp = tempfile::tempdir().unwrap();
    let mut uri = PackageUri::create("package://example.test/a@1.0.0").unwrap();
    uri.base_path = "/../../../outside".into();
    uri.path = "/../../../outside@1.0.0".into();
    let dependency = PackageDependency {
        uri,
        checksums: None,
    };
    let result = PackageManager::new(PklCli::default()).get_library_roots(&dependency, temp.path());
    assert!(matches!(
        result,
        Err(pkl_project::package_manager::PackageError::InvalidCachePath(_))
    ));
}
