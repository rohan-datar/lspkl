//! Tests for the DTO layer.

use lspkl_project::dto::{
    Checksums, LocalDependency, PackageUri, PklProject, ProjectDeps, RemoteDependency,
    ResolvedDependency,
};

/// A realistic `PklProject.deps.json` fragment, modeled after what
/// `pkl project resolve` emits.
const DEPS_JSON: &str = r#"{
  "schemaVersion": 1,
  "resolvedDependencies": {
    "package://example.com/shape@1": {
      "type": "remote",
      "uri": "projectpackage://example.com/shape@1.2.3::sha256:abcdef1234567890",
      "checksums": { "sha256": "abcdef1234567890" }
    },
    "package://localhost/utils@0": {
      "type": "local",
      "uri": "projectpackage://localhost/utils@0.0.1",
      "path": "../utils"
    }
  }
}"#;

#[test]
fn package_uri_parses_authority_path_version_checksum() {
    let uri =
        PackageUri::create("package://example.com/bar/foo@2.0.0::sha256:cafebabe").expect("valid");
    assert_eq!(uri.authority, "example.com");
    assert_eq!(uri.base_path, "/bar/foo");
    assert_eq!(uri.path, "/bar/foo@2.0.0::sha256:cafebabe");
    assert_eq!(uri.version.to_string(), "2.0.0");
    assert_eq!(
        uri.checksums,
        Some(Checksums {
            sha256: "cafebabe".into()
        })
    );
}

#[test]
fn package_uri_without_checksum_round_trips() {
    let uri = PackageUri::create("package://example.com/bar/foo@0.27.1").expect("valid");
    assert_eq!(
        uri.to_display_string(),
        "package://example.com/bar/foo@0.27.1"
    );
}

#[test]
fn package_cache_candidates_prefer_package_2_and_strip_checksum_suffix() {
    let uri = PackageUri::create("package://example.com/bar/foo@1.2.3::sha256:cafebabe").unwrap();
    assert_eq!(
        uri.relative_metadata_files(),
        [
            "package-2/example.com/bar/foo@1.2.3/foo@1.2.3.json",
            "package-1/example.com/bar/foo@1.2.3/foo@1.2.3.json",
        ]
    );
    assert_eq!(
        uri.relative_zip_files(),
        [
            "package-2/example.com/bar/foo@1.2.3/foo@1.2.3.zip",
            "package-1/example.com/bar/foo@1.2.3/foo@1.2.3.zip",
        ]
    );
}

#[test]
fn package_cache_candidates_encode_reserved_filename_characters() {
    let uri = PackageUri::create("package://example.com/a:b/foo(thing)@1.2.3").unwrap();
    assert_eq!(
        uri.relative_zip_files()[0],
        "package-2/example.com/a(3a)b/foo((thing)@1.2.3/foo((thing)@1.2.3.zip"
    );
    assert_eq!(
        uri.relative_zip_files()[1],
        "package-1/example.com/a:b/foo(thing)@1.2.3/foo(thing)@1.2.3.zip"
    );
}

#[test]
fn package_uri_equality_includes_version() {
    let a = PackageUri::create("package://example.com/foo@1.0.0").unwrap();
    let b = PackageUri::create("package://example.com/foo@2.0.0").unwrap();
    assert_ne!(a, b);
}

#[test]
fn package_uri_rejects_missing_version() {
    assert!(PackageUri::create("package://example.com/foo").is_err());
}

#[test]
fn package_uri_rejects_unsupported_checksum_algorithm() {
    assert!(PackageUri::create("package://example.com/foo@1.0.0::md5:deadbeef").is_err());
}

#[test]
fn package_uri_serde_uses_a_string() {
    let uri = PackageUri::create("package://example.com/foo@1.2.3").unwrap();
    let json = serde_json::to_string(&uri).unwrap();
    assert_eq!(json, r#""package://example.com/foo@1.2.3""#);

    let decoded: PackageUri = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, uri);
}

#[test]
fn deps_json_parses_local_and_remote() {
    let deps: ProjectDeps = serde_json::from_str(DEPS_JSON).expect("parses");
    assert_eq!(deps.schema_version, 1);
    assert_eq!(deps.resolved_dependencies.len(), 2);

    match deps.get_resolved_dependency("package://example.com/shape@1.2.3") {
        Some(ResolvedDependency::Remote(RemoteDependency { checksums, .. })) => {
            assert_eq!(
                checksums.as_ref().map(|c| c.sha256.as_str()),
                Some("abcdef1234567890")
            );
        }
        other => panic!("expected remote dep for shape, got {other:?}"),
    }
}

#[test]
fn deps_json_prefix_lookup_finds_by_package_uri_prefix() {
    let deps: ProjectDeps = serde_json::from_str(DEPS_JSON).expect("parses");
    // Resolution keys identify a package and major version; the resolved URI
    // contains the complete version.
    let hit = deps.get_resolved_dependency("package://example.com/shape@1.9.9");
    assert!(
        hit.is_some(),
        "prefix lookup should match by resolved URI prefix"
    );
}

#[test]
fn local_dependency_serializes_with_lowercase_tag() {
    let dep = ResolvedDependency::Local(LocalDependency {
        uri: PackageUri::create("package://localhost/utils@0.0.1").unwrap(),
        path: "../other".into(),
    });
    let json = serde_json::to_value(&dep).unwrap();
    assert_eq!(json["type"], "local");
    assert_eq!(json["path"], "../other");
}

#[test]
fn project_metadata_parses_from_cli_output() {
    let metadata_json = r#"[{
        "projectFileUri": "file:///work/demo/PklProject",
        "packageUri": null,
        "declaredDependencies": {
            "@clip/shape": "package://example.com/shape@1.2.3"
        },
        "evaluatorSettings": {
            "modulePath": ["/opt/modules", "libs.jar"]
        },
        "someFutureField": true
    }]"#;
    let projects = PklProject::parse_metadata(metadata_json).expect("parses");
    assert_eq!(projects.len(), 1);
    assert_eq!(
        projects[0].metadata.declared_dependencies.len(),
        1,
        "unknown fields must be ignored"
    );
}

#[test]
fn missing_local_dependency_keeps_declared_identity_and_path() {
    use lspkl_project::dto::{Dependency, resolved_to_dependency};
    let root = tempfile::tempdir().unwrap();
    let uri = PackageUri::create("package://localhost/utils@0.0.1").unwrap();
    let dep = ResolvedDependency::Local(LocalDependency {
        uri: uri.clone(),
        path: "missing".into(),
    });
    let Dependency::LocalProject(local) = resolved_to_dependency(&dep, root.path()) else {
        panic!("declared local dependency must not become a package");
    };
    assert_eq!(local.uri, uri);
    assert_eq!(local.project_dir, root.path().join("missing"));
    assert!(!local.project_dir.exists());
}
