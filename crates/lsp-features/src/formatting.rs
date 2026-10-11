//! Formatting: shell out to `pkl format` and return a whole-document edit.

use tower_lsp_server::lsp_types::TextEdit;

use std::io;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::MAX_RANGE;
/// Format `source` with `pkl format` and return a single whole-document
/// `TextEdit` covering `0..end-of-file`. Returns an empty `Vec` when the
/// formatter produced no changes.
pub fn formatting(
    source: &str,
    cli_path: &std::path::Path,
    grammar_version: Option<&str>,
) -> io::Result<Vec<TextEdit>> {
    let mut command = Command::new(cli_path);
    command.arg("format");
    if let Some(version) = grammar_version {
        command.arg("--grammar-version").arg(version);
    }
    let mut child = command
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    child.stdin.as_mut().unwrap().write_all(source.as_bytes())?;
    let output = child.wait_with_output()?;

    if output.status.success() {
        // indicates document already formatted
        Ok(vec![])
    } else if output.status.code() == Some(11) {
        // indicates changes were made
        let formatted = String::from_utf8(output.stdout)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        Ok(vec![TextEdit::new(MAX_RANGE, formatted)])
    } else {
        // formatting failed
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::other(format!("pkl format failed: {stderr}")))
    }
}
