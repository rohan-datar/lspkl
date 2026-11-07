//! Invoking the `pkl` CLI.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

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

    /// Probe on an I/O worker only. Capture in an anonymous file rather than a
    /// pipe: even a broken wrapper retaining stdout in a descendant cannot hold
    /// startup hostage. Timeout kills/reaps the direct child and means unknown.
    fn version(&self) -> Option<PklVersion> {
        let mut output = tempfile::tempfile().ok()?;
        let binary = self
            .binary_path
            .as_deref()
            .unwrap_or_else(|| Path::new("pkl"));
        let mut child = Command::new(binary)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(output.try_clone().ok()?)
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(Some(_)) => return None,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
        output.seek(SeekFrom::Start(0)).ok()?;
        let mut text = String::new();
        output.take(4096).read_to_string(&mut text).ok()?;
        PklVersion::parse(&text)
    }

    /// `pkl project resolve <dirs...>`
    pub fn resolve_project(
        &self,
        project_dirs: &[PathBuf],
        cache_dir: &Path,
    ) -> Result<String, PklCliError> {
        let args = [
            OsStr::new("project"),
            OsStr::new("resolve"),
            OsStr::new("--cache-dir"),
            cache_dir.as_os_str(),
        ]
        .into_iter()
        .chain(project_dirs.iter().map(|path| path.as_os_str()));
        self.execute(args)
    }

    /// Evaluate one or more modules and return the CLI's stdout.
    pub fn eval<I, S>(
        &self,
        modules: I,
        cache_dir: &Path,
        expression: Option<&str>,
        module_output_separator: Option<&str>,
    ) -> Result<String, PklCliError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut args: Vec<OsString> = vec![
            OsString::from("eval"),
            OsString::from("--cache-dir"),
            cache_dir.as_os_str().to_owned(),
        ];

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

/// Version relevant to the CLI's package-cache convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PklVersion(u32, u32, u32);

impl PklVersion {
    pub fn parse(output: &str) -> Option<Self> {
        let mut words = output.split_whitespace();
        if words.next()? != "Pkl" {
            return None;
        }
        let mut parts = words.next()?.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.split('-').next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self(major, minor, patch))
    }
}

/// Captured environment; constructing it does not inspect the filesystem.
#[derive(Debug, Clone)]
pub struct CacheDirectories {
    pub modern: PathBuf,
    pub legacy: PathBuf,
}

impl CacheDirectories {
    pub fn from_env() -> Self {
        Self::from_environment(cfg!(windows), |name| std::env::var_os(name))
    }

    fn from_environment(windows: bool, env: impl Fn(&str) -> Option<OsString>) -> Self {
        let nonempty = |name| {
            env(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let home = nonempty("HOME")
            .or_else(|| nonempty("USERPROFILE"))
            .unwrap_or_else(|| PathBuf::from("."));
        let modern = nonempty("XDG_CACHE_HOME")
            .map(|path| path.join("pkl"))
            .or_else(|| {
                windows
                    .then(|| nonempty("LOCALAPPDATA").map(|path| path.join("pkl/Cache")))
                    .flatten()
            })
            .unwrap_or_else(|| home.join(".cache/pkl"));
        Self {
            modern,
            legacy: home.join(".pkl/cache"),
        }
    }

    /// Call only on an I/O worker: unknown versions inspect cache existence.
    pub fn select(&self, version: Option<PklVersion>) -> PathBuf {
        self.select_with(version, |path| path.is_dir())
    }

    fn select_with(&self, version: Option<PklVersion>, exists: impl Fn(&Path) -> bool) -> PathBuf {
        match version {
            Some(version) if version < PklVersion(0, 33, 0) => self.legacy.clone(),
            Some(_) => self.modern.clone(),
            None if !exists(&self.modern) && exists(&self.legacy) => self.legacy.clone(),
            None => self.modern.clone(),
        }
    }
}

/// Session-local, bounded cache. Only I/O workers call `detect`; no database
/// handle or main-loop thread waits on the lock or the subprocess.
#[derive(Debug, Default)]
pub struct CliVersionCache(Mutex<HashMap<PathBuf, Option<PklVersion>>>);

impl CliVersionCache {
    pub fn detect(&self, binary: &Path) -> Option<PklVersion> {
        let mut cache = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(version) = cache.get(binary) {
            return *version;
        }
        let version = PklCli {
            binary_path: Some(binary.to_owned()),
        }
        .version();
        // Settings can supply arbitrarily many CLI paths over a long session.
        if cache.len() >= 32 {
            cache.clear();
        }
        cache.insert(binary.to_owned(), version);
        version
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn parses_cli_versions() {
        assert_eq!(
            PklVersion::parse("Pkl 0.32.1 (macOS aarch64, native)"),
            Some(PklVersion(0, 32, 1))
        );
        assert_eq!(
            PklVersion::parse("Pkl 0.33.0 (macOS aarch64, native)\n"),
            Some(PklVersion(0, 33, 0))
        );
        assert_eq!(
            PklVersion::parse("Pkl 0.34.0-dev"),
            Some(PklVersion(0, 34, 0))
        );
        for invalid in [
            "",
            "unknown",
            "Pkl nope",
            "Pkl 0.33",
            "Pkl 0.33.x",
            "Pkl 0.33.0.1",
        ] {
            assert_eq!(PklVersion::parse(invalid), None);
        }
    }

    fn directories(windows: bool, vars: &[(&str, &str)]) -> CacheDirectories {
        CacheDirectories::from_environment(windows, |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        })
    }

    #[test]
    fn platform_cache_directories_use_injected_environment() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let home = home.to_str().unwrap();
        let local = root.path().join("local");
        let local = local.to_str().unwrap();
        let xdg = root.path().join("xdg");
        let xdg = xdg.to_str().unwrap();
        for windows in [false, true] {
            let dirs = directories(
                windows,
                &[
                    ("HOME", home),
                    ("XDG_CACHE_HOME", xdg),
                    ("LOCALAPPDATA", local),
                ],
            );
            assert_eq!(dirs.modern, Path::new(xdg).join("pkl"));
            assert_eq!(dirs.legacy, Path::new(home).join(".pkl/cache"));
        }
        assert_eq!(
            directories(false, &[("HOME", home)]).modern,
            Path::new(home).join(".cache/pkl")
        );
        assert_eq!(
            directories(true, &[("USERPROFILE", home), ("LOCALAPPDATA", local)]).modern,
            Path::new(local).join("pkl/Cache")
        );
        assert_eq!(
            directories(
                true,
                &[
                    ("USERPROFILE", home),
                    ("XDG_CACHE_HOME", ""),
                    ("LOCALAPPDATA", local)
                ]
            )
            .modern,
            Path::new(local).join("pkl/Cache")
        );
    }

    #[test]
    fn cache_selection_matrix() {
        let dirs = CacheDirectories {
            modern: "modern".into(),
            legacy: "legacy".into(),
        };
        for modern_exists in [false, true] {
            for legacy_exists in [false, true] {
                let exists = |path: &Path| {
                    if path == dirs.modern {
                        modern_exists
                    } else {
                        legacy_exists
                    }
                };
                assert_eq!(
                    dirs.select_with(Some(PklVersion(0, 32, 1)), exists),
                    dirs.legacy
                );
                assert_eq!(
                    dirs.select_with(Some(PklVersion(0, 33, 0)), exists),
                    dirs.modern
                );
                assert_eq!(
                    dirs.select_with(Some(PklVersion(1, 0, 0)), exists),
                    dirs.modern
                );
                assert_eq!(
                    dirs.select_with(None, exists),
                    if !modern_exists && legacy_exists {
                        &dirs.legacy
                    } else {
                        &dirs.modern
                    }
                    .clone()
                );
            }
        }
    }
}
