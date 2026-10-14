//! Invoking the `pkl` CLI.

use std::ffi::{OsStr, OsString};
use std::io::ErrorKind;
use std::iter::once;
use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;
use tracing::info;

#[derive(Debug, Error)]
pub enum PklCliError {
    #[error("the `pkl` CLI is not installed or not on PATH")]
    NotFound,
    #[error("`pkl {args:?}` failed with exit code {code:?}:\n{stderr}")]
    Failed {
        args: Vec<String>,
        code: Option<i32>,
        stderr: String,
    },
    #[error("I/O error talking to the `pkl` CLI: {0}")]
    Io(#[from] std::io::Error),
}

/// Handle for invoking the `pkl` CLI.
#[derive(Debug, Clone, Default)]
pub struct PklCli {
    /// Explicit binary path; `None` = resolve `pkl` from `$PATH`.
    pub binary_path: Option<PathBuf>,
}

impl PklCli {
    /// Run a `pkl` subcommand and return stdout on success.
    pub fn execute<I, S>(&self, args: I) -> Result<String, PklCliError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let pkl_path = self
            .binary_path
            .as_deref()
            .unwrap_or_else(|| Path::new("pkl"));

        let args: Vec<OsString> = args
            .into_iter()
            .map(|arg| arg.as_ref().to_owned())
            .collect();

        info!(program = %pkl_path.display(), ?args, "Spawning Command");

        let output =
            Command::new(pkl_path)
                .args(&args)
                .output()
                .map_err(|err| match err.kind() {
                    ErrorKind::NotFound => PklCliError::NotFound,
                    _ => PklCliError::Io(err),
                })?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(PklCliError::Failed {
                args: args.iter().map(|arg| arg.display().to_string()).collect(),
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
    }

    /// `pkl project resolve <dirs...>`
    pub fn resolve_project(&self, project_dirs: &[PathBuf]) -> Result<String, PklCliError> {
        let args = once(OsStr::new("project"))
            .chain(once(OsStr::new("resolve")))
            .chain(project_dirs.iter().map(|path| path.as_os_str()));
        self.execute(args)
    }

    /// Evaluate one or more modules and return the CLI's stdout.
    pub fn eval<I, S>(
        &self,
        modules: I,
        expression: Option<&str>,
        module_output_separator: Option<&str>,
    ) -> Result<String, PklCliError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut args: Vec<OsString> = vec![OsString::from("eval")];

        if let Some(expression) = expression {
            args.push(OsString::from("-x"));
            args.push(OsString::from(expression));
        }
        if let Some(separator) = module_output_separator {
            args.push(OsString::from("--module-output-separator"));
            args.push(OsString::from(separator));
        }

        args.extend(modules.into_iter().map(|module| module.as_ref().to_owned()));

        self.execute(args)
    }

    /// `pkl package download <package-uri>` — populates the module cache.
    pub fn download_package(&self, package_uri: &str, cache_dir: &Path) -> Result<(), PklCliError> {
        let _ = self.execute([
            OsStr::new("download-package"),
            OsStr::new("--no-transitive"),
            OsStr::new("--cache-dir"),
            cache_dir.as_os_str(),
            OsStr::new(package_uri),
        ])?;
        Ok(())
    }
}
