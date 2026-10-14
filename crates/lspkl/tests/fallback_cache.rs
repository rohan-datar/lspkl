//! Run environment-sensitive cache regressions in a fresh test process.
#![cfg(unix)]
mod support;

use lsp_types::*;
use lspkl::{Workspace, handlers};
use lspkl_db::settings::WorkspaceSettings;
use lspkl_project::dto::{PackageMetadata, PackageUri};
use std::{collections::BTreeMap, fs, io::Write, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn download_and_import_use_the_same_fallback_cache() {
    if std::env::var_os("LSPKL_CACHE_TEST_CHILD").is_some() {
        child();
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    // No process-global environment mutation: Rust's test runner may run all
    // other tests concurrently while these children have isolated environments.
    for home in [true, false] {
        let root = temp
            .path()
            .join(if home { "home-present" } else { "home-missing" });
        fs::create_dir(&root).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "download_and_import_use_the_same_fallback_cache",
                "--nocapture",
            ])
            .env("LSPKL_CACHE_TEST_CHILD", "1")
            .env("LSPKL_CACHE_TEST_ROOT", &root)
            .env_remove("USERPROFILE")
            .env("XDG_CACHE_HOME", root.join("xdg"))
            .current_dir(&root);
        if home {
            command.env("HOME", root.join("home"));
        } else {
            command.env_remove("HOME");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "child failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
fn child() {
    let root = std::path::PathBuf::from(std::env::var_os("LSPKL_CACHE_TEST_ROOT").unwrap());
    let calls = root.join("arguments");
    let cli = root.join("pkl");
    let quote = |path: &std::path::Path| {
        format!(
            "'{}'",
            path.to_string_lossy().replace('\u{0027}', "'\"'\"'")
        )
    };
    fs::write(
        &cli,
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\n", quote(&calls)),
    )
    .unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    let mut workspace = Workspace::new();
    workspace.apply_settings(
        WorkspaceSettings::parse(serde_json::json!({"cli": {"path": cli}})).unwrap(),
    );
    let snapshot = workspace.snapshot();
    assert!(snapshot.settings.resolved_cache_dir.is_none());
    let cache = snapshot.settings.effective_cache_dir().to_path_buf();
    let expected = if let Some(home) = std::env::var_os("HOME") {
        std::path::PathBuf::from(home).join(".pkl/cache")
    } else {
        lspkl_vfs::default_cache()
    };
    assert_eq!(cache, expected);
    let package = PackageUri::create("package://cache-regression.test/library@1.0.0").unwrap();
    let command =
        handlers::commands::prepare_download_package(&snapshot, package.to_display_string())
            .unwrap();
    drop(snapshot);
    handlers::commands::execute_command(command).unwrap();
    let arguments = fs::read_to_string(&calls).unwrap();
    assert_eq!(
        arguments.lines().collect::<Vec<_>>(),
        [
            "download-package",
            "--no-transitive",
            "--cache-dir",
            cache.to_str().unwrap(),
            "package://cache-regression.test/library@1.0.0"
        ]
    );

    // Only this cache contains the package; successful semantic navigation is
    // evidence that import routing read the same directory passed to the CLI.
    let metadata = PackageMetadata {
        name: "library".into(),
        version: "1.0.0".into(),
        package_zip_url: "https://cache-regression.test/package.zip".into(),
        package_uri: package.clone(),
        dependencies: BTreeMap::new(),
    };
    let metadata_path = cache.join(&package.relative_metadata_files()[0]);
    fs::create_dir_all(metadata_path.parent().unwrap()).unwrap();
    fs::write(metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let archive_path = cache.join(&package.relative_zip_files()[0]);
    let mut archive = zip::ZipWriter::new(fs::File::create(&archive_path).unwrap());
    archive
        .start_file("lib.pkl", zip::write::SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"value: Int = 42\n").unwrap();
    archive.finish().unwrap();
    let uri: Uri = url::Url::from_file_path(root.join("main.pkl"))
        .unwrap()
        .as_str()
        .parse()
        .unwrap();
    handlers::lifecycle::did_open(&mut workspace, DidOpenTextDocumentParams { text_document: TextDocumentItem { uri: uri.clone(), language_id: "pkl".into(), version: 1, text: "import \"package://cache-regression.test/library@1.0.0#/lib.pkl\" as library\nanswer = library.value\n".into() } }).unwrap();
    support::preload(&mut workspace);
    let snapshot = workspace.snapshot();
    let result = handlers::documents::definition(
        &snapshot,
        GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position::new(1, 17),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        },
    )
    .unwrap()
    .unwrap();
    let GotoDefinitionResponse::Scalar(location) = result else {
        panic!("expected package member definition")
    };
    assert_eq!(location.range.start, Position::new(0, 0));
    let request = handlers::documents::prepare_file_contents(
        &snapshot,
        TextDocumentIdentifier { uri: location.uri },
    )
    .unwrap();
    drop(snapshot);
    assert_eq!(
        handlers::documents::file_contents(request)
            .unwrap()
            .source
            .contents(),
        "value: Int = 42\n"
    );
}
