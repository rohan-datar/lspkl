//! Hermetic tests for `PklCli` argument construction.

#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pkl_project::cli::{PklCli, PklCliError};

// Executing scripts while another test creates one can return ETXTBSY on
// some temporary filesystems; keep this integration test's stubs serial.
static SCRIPT_LOCK: Mutex<()> = Mutex::new(());

/// Write a stand-in `pkl` executable with the supplied shell body.
fn script_cli(body: &str) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let staged = temp.path().join("pkl-stub.tmp");
    let binary = temp.path().join("pkl-stub");
    let mut file = fs::File::create(&staged).unwrap();
    writeln!(file, "#!/bin/sh\n{body}").unwrap();
    file.sync_all().unwrap();
    drop(file);

    let mut permissions = fs::metadata(&staged).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&staged, permissions).unwrap();
    fs::rename(staged, &binary).unwrap();

    (temp, binary)
}

/// Write a stand-in `pkl` executable that prints one argument per line.
fn echo_cli() -> (tempfile::TempDir, PathBuf) {
    script_cli(r#"printf '%s\n' "$@""#)
}

#[test]
fn eval_places_options_before_modules() {
    let _guard = SCRIPT_LOCK.lock().unwrap();
    let (_temp, binary) = echo_cli();
    let cli = PklCli {
        binary_path: Some(binary),
    };
    let modules = [
        Path::new("/workspace/first.pkl"),
        Path::new("/workspace/path with spaces/second.pkl"),
    ];

    let output = cli
        .eval(modules, Some("module.dependencies.toMap()"), Some(", "))
        .expect("stub command succeeds");

    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        [
            "eval",
            "-x",
            "module.dependencies.toMap()",
            "--module-output-separator",
            ", ",
            "/workspace/first.pkl",
            "/workspace/path with spaces/second.pkl",
        ]
    );
}

#[test]
fn eval_omits_absent_options() {
    let _guard = SCRIPT_LOCK.lock().unwrap();
    let (_temp, binary) = echo_cli();
    let cli = PklCli {
        binary_path: Some(binary),
    };

    let output = cli
        .eval(["package://example.com/demo@1.0.0"], None, None)
        .expect("stub command succeeds");

    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        ["eval", "package://example.com/demo@1.0.0"]
    );
}

#[test]
fn resolve_project_passes_subcommand_and_paths() {
    let _guard = SCRIPT_LOCK.lock().unwrap();
    let (_temp, binary) = echo_cli();
    let cli = PklCli {
        binary_path: Some(binary),
    };
    let project_dirs = [
        PathBuf::from("/workspace/first"),
        PathBuf::from("/workspace/path with spaces/second"),
    ];

    let output = cli
        .resolve_project(&project_dirs)
        .expect("stub command succeeds");

    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        [
            "project",
            "resolve",
            "/workspace/first",
            "/workspace/path with spaces/second",
        ]
    );
}

#[test]
fn download_package_uses_no_transitive_flag() {
    let _guard = SCRIPT_LOCK.lock().unwrap();
    let log_dir = tempfile::tempdir().unwrap();
    let log = log_dir.path().join("args");
    let body = format!("printf '%s\\n' \"$@\" > {}", shell_quote(log.as_os_str()));
    let (_temp, binary) = script_cli(&body);
    let cli = PklCli {
        binary_path: Some(binary),
    };

    cli.download_package(
        "package://example.com/demo@1.0.0::sha256:cafebabe",
        Path::new("/tmp/pkl cache"),
    )
    .expect("stub command succeeds");

    assert_eq!(
        fs::read_to_string(log).unwrap().lines().collect::<Vec<_>>(),
        [
            "download-package",
            "--no-transitive",
            "--cache-dir",
            "/tmp/pkl cache",
            "package://example.com/demo@1.0.0::sha256:cafebabe",
        ]
    );
}
fn shell_quote(value: &std::ffi::OsStr) -> String {
    format!("'{}'", value.to_string_lossy().replace('\'', "'\\''"))
}

#[test]
fn execute_reports_exit_code_stderr_and_arguments() {
    let _guard = SCRIPT_LOCK.lock().unwrap();
    let (_temp, binary) = script_cli("printf 'bad input\\n' >&2\nexit 7");
    let cli = PklCli {
        binary_path: Some(binary),
    };

    match cli.execute(["eval", "broken.pkl"]) {
        Err(PklCliError::Failed { args, code, stderr }) => {
            assert_eq!(args, ["eval", "broken.pkl"]);
            assert_eq!(code, Some(7));
            assert_eq!(stderr, "bad input\n");
        }
        other => panic!("expected command failure, got {other:?}"),
    }
}

#[test]
fn execute_reports_missing_binary() {
    let _guard = SCRIPT_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let cli = PklCli {
        binary_path: Some(temp.path().join("does-not-exist")),
    };

    assert!(matches!(
        cli.execute(["version"]),
        Err(PklCliError::NotFound)
    ));
}
