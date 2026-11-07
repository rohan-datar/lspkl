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
    let expected = root.join("xdg/pkl");
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

#[path = "support/memory.rs"]
mod memory;

#[test]
fn cli_version_selects_cache_and_path_changes_reprobe() {
    if std::env::var_os("LSPKL_VERSION_CACHE_CHILD").is_some() {
        version_child();
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    for scenario in [
        "modern-xdg",
        "modern-home",
        "legacy",
        "unknown-modern",
        "unknown-legacy",
        "unknown-both",
        "unknown-neither",
        "override",
        "switch",
        "timeout",
        "slow-probe",
    ] {
        let root = temp.path().join(scenario);
        fs::create_dir(&root).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "cli_version_selects_cache_and_path_changes_reprobe",
                "--nocapture",
            ])
            .env("LSPKL_VERSION_CACHE_CHILD", scenario)
            .env("LSPKL_CACHE_TEST_ROOT", &root)
            .env("HOME", root.join("home"))
            .env_remove("USERPROFILE")
            .env_remove("LOCALAPPDATA")
            .env_remove("XDG_CACHE_HOME")
            .current_dir(&root);
        if scenario != "modern-home" {
            command.env("XDG_CACHE_HOME", root.join("xdg"));
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{scenario}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn version_child() {
    use serde_json::json;
    use std::time::{Duration, Instant};
    let scenario = std::env::var("LSPKL_VERSION_CACHE_CHILD").unwrap();
    let root = std::path::PathBuf::from(std::env::var_os("LSPKL_CACHE_TEST_ROOT").unwrap());
    let modern = if scenario == "modern-home" {
        root.join("home/.cache/pkl")
    } else {
        root.join("xdg/pkl")
    };
    let legacy = root.join("home/.pkl/cache");
    if ["unknown-modern", "unknown-both"].contains(&scenario.as_str()) {
        fs::create_dir_all(&modern).unwrap();
    }
    if [
        "unknown-legacy",
        "unknown-both",
        "modern-xdg",
        "timeout",
        "slow-probe",
    ]
    .contains(&scenario.as_str())
    {
        fs::create_dir_all(&legacy).unwrap();
    }
    let calls = root.join("cache-argument");
    let probes = root.join("probes");
    let release = root.join("release-probe");
    let finished = root.join("finished-probe");
    let quote =
        |path: &std::path::Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"));
    let make_cli = |name: &str, version: &str| {
        let path = root.join(name);
        let answer = if version == "slow-probe" {
            format!(
                "while [ ! -e {} ]; do /bin/sleep 0.01; done; echo done > {}; echo 'Pkl 0.33.0 (native)'; exit 0",
                quote(&release),
                quote(&finished)
            )
        } else if version == "timeout" {
            format!(
                "echo $$ > {}; exec /bin/sleep 30",
                quote(&root.join("probe-pid"))
            )
        } else {
            format!("printf '%s\\n' '{version}'; exit 0")
        };
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf '%s\\n' '{}' >> {}; {}; fi\nif [ \"$1\" = download-package ]; then printf '%s' \"$4\" > {}; exit 0; fi\nexit 9\n",
            name,
            quote(&probes),
            answer,
            quote(&calls)
        );
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    let version = if ["timeout", "slow-probe"].contains(&scenario.as_str()) {
        scenario.as_str()
    } else if scenario.starts_with("unknown") {
        "not a version"
    } else if scenario == "legacy" {
        "Pkl 0.32.1 (native)"
    } else {
        "Pkl 0.33.0 (native)"
    };
    let cli = make_cli("pkl-modern", version);
    let mut options = json!({"cli":{"path":cli}});
    if scenario == "override" {
        options["cacheDir"] = json!(root.join("explicit"));
    }
    let started = Instant::now();
    let mut peer = memory::Peer::start(
        Workspace::new(),
        json!({"capabilities":{},"initializationOptions":options}),
    );
    let expected = if scenario == "override" {
        root.join("explicit")
    } else if ["legacy", "unknown-legacy", "timeout"].contains(&scenario.as_str()) {
        legacy.clone()
    } else {
        modern.clone()
    };
    let download = |peer: &mut memory::Peer| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let response = peer.request(
                "pkl/downloadPackage",
                json!("package://cache-regression.test/library@1.0.0"),
            );
            if response["error"]["code"] == -32801 {
                assert!(Instant::now() < deadline, "cache did not settle");
                continue;
            }
            assert!(response.get("error").is_none(), "{response}");
            break;
        }
        std::path::PathBuf::from(fs::read_to_string(&calls).unwrap())
    };
    if ["timeout", "slow-probe"].contains(&scenario.as_str()) {
        assert_eq!(download(&mut peer), legacy);
    } else {
        let deadline = Instant::now() + Duration::from_secs(5);
        while download(&mut peer) != expected {
            assert!(Instant::now() < deadline, "cache did not settle");
        }
    }
    if ["timeout", "slow-probe"].contains(&scenario.as_str()) {
        peer.open("inmemory:///timeout.pkl", 1, "value = 1\n");
        loop {
            if let lsp_server::Message::Notification(notification) = peer.message()
                && notification.method == "textDocument/publishDiagnostics"
                && notification.params["version"] == 1
            {
                assert_eq!(notification.params["diagnostics"], json!([]));
                break;
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "CLI probe delayed startup diagnostics"
        );
    }
    if scenario == "timeout" {
        let pid = fs::read_to_string(root.join("probe-pid")).unwrap();
        let alive = root.join("probe-alive");
        fs::write(&alive, "#!/bin/sh\nkill -0 \"$1\" 2>/dev/null\n").unwrap();
        fs::set_permissions(&alive, fs::Permissions::from_mode(0o755)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Command::new(&alive)
            .arg(pid.trim())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
        {
            assert!(
                Instant::now() < deadline,
                "timed-out version child was not killed"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    if scenario == "slow-probe" {
        assert!(
            !finished.exists(),
            "diagnostics must arrive before probe finishes"
        );
        fs::write(&release, "").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while download(&mut peer) != modern {
            assert!(
                Instant::now() < deadline,
                "version-derived cache never applied"
            );
        }
        assert!(finished.exists());
    }
    if scenario == "switch" {
        let old = make_cli("pkl-legacy", "Pkl 0.32.1 (native)");
        for (path, expected) in [(old, legacy), (cli, modern)] {
            peer.notify(
                "workspace/didChangeConfiguration",
                json!({"settings":{"cli":{"path":path}}}),
            );
            let deadline = Instant::now() + Duration::from_secs(5);
            while download(&mut peer) != expected {
                assert!(Instant::now() < deadline, "cache did not update");
            }
        }
        assert_eq!(
            fs::read_to_string(probes)
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            ["pkl-modern", "pkl-legacy"]
        );
    } else if scenario == "override" {
        assert!(!probes.exists(), "explicit cache does not need CLI probing");
    } else {
        assert_eq!(fs::read_to_string(probes).unwrap().lines().count(), 1);
    }
    peer.stop();
}
