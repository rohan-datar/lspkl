//! Consumer-visible Pkl subprocess failures.

#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Mutex;

use lspkl_project::cli::{PklCli, PklCliError};

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
