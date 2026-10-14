//! Data types for `.pkl-project` metadata and `PklProject.deps.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lspkl_vfs::url_to_path;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json;
use thiserror::Error;

use fluent_uri::Uri;

// ---------------------------------------------------------------------------
// PackageUri
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum PackageUriError {
    #[error("package URI `{0}` is missing a name")]
    MissingName(String),
    #[error("package URI `{0}` is missing an authority")]
    MissingAuthority(String),
    #[error("package URI `{0}` has no `@version` component")]
    MissingVersion(String),
    #[error("version `{0}` in package URI `{1}` is not valid semver")]
    BadVersion(String, String),

    #[error("invalid URI: {0}")]
    ParseError(#[from] fluent_uri::ParseError),

    #[error("invalid checksum: `{0}` in package URI `{1}`")]
    BadChecksum(String, String),
    #[error("package URI `{0}` has an invalid scheme or path")]
    InvalidIdentity(String),
}

fn encode_path(path: &str) -> String {
    if path.is_empty() {
        return path.to_owned();
    }
    let mut encoded = String::with_capacity(path.len());
    for ch in path.chars() {
        match ch {
            '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*' => {
                let byte = ch as u32 as u8;
                let signed = byte as i8;
                encoded.push_str(&format!("({:x})", signed));
            }
            '(' => encoded.push_str("(("),
            _ => encoded.push(ch),
        }
    }
    encoded
}

/// A `package://<authority>/<name>@<version>[::sha256:<checksum>]` URI.
#[derive(Debug, Clone)]
pub struct PackageUri {
    pub authority: String,
    /// Path without the `@version` suffix.
    pub base_path: String,
    pub path: String,
    pub version: Version,
    pub checksums: Option<Checksums>,
}

impl PackageUri {
    /// Parse a package URI string.
    pub fn create(input: &str) -> Result<Self, PackageUriError> {
        let uri = Uri::parse(input).map_err(PackageUriError::ParseError)?;
        if !matches!(uri.scheme().as_str(), "package" | "projectpackage")
            || uri
                .path()
                .as_str()
                .split('/')
                .any(|part| matches!(part, "." | ".."))
        {
            return Err(PackageUriError::InvalidIdentity(input.to_owned()));
        }

        let Some(authority) = uri.authority() else {
            return Err(PackageUriError::MissingAuthority(input.to_owned()));
        };

        let path = uri.path().as_str();

        let Some((base_path, version_and_sum)) = path.rsplit_once("@") else {
            return Err(PackageUriError::MissingVersion(input.to_owned()));
        };

        let (version, checksum) = version_and_sum
            .split_once("::")
            .map_or((version_and_sum, None), |(v, c)| (v, Some(c)));

        let Ok(version) = Version::parse(version) else {
            return Err(PackageUriError::BadVersion(
                version.to_owned(),
                input.to_owned(),
            ));
        };

        let checksums = match checksum {
            None => None,
            Some(checksum) => {
                let Some((algorithm, value)) = checksum.split_once(":") else {
                    return Err(PackageUriError::BadChecksum(
                        checksum.to_owned(),
                        input.to_owned(),
                    ));
                };
                if algorithm != "sha256" {
                    return Err(PackageUriError::BadChecksum(
                        checksum.to_owned(),
                        input.to_owned(),
                    ));
                }
                Some(Checksums {
                    sha256: value.to_owned(),
                })
            }
        };

        Ok(PackageUri {
            authority: authority.as_str().to_owned(),
            base_path: base_path.to_owned(),
            path: path.to_owned(),
            version,
            checksums,
        })
    }

    /// `<authority><basePath>@<version>` — no checksum suffix.
    pub fn to_display_string(&self) -> String {
        format!(
            "package://{}{}@{}",
            self.authority, self.base_path, self.version
        )
    }

    fn package_name(&self) -> String {
        let (_, last_segment) = self.path.rsplit_once("/").unwrap_or(("", &self.base_path));
        let (name_with_version, _) = last_segment.rsplit_once("::").unwrap_or((last_segment, ""));
        format!(
            "{}{}@{}/{}",
            self.authority, self.base_path, self.version, name_with_version
        )
    }

    pub fn relative_metadata_files(&self) -> Vec<String> {
        let filename_base = self.package_name();
        vec![
            encode_path(&format!("package-2/{}.json", filename_base)),
            format!("package-1/{}.json", filename_base),
        ]
    }

    pub fn relative_zip_files(&self) -> Vec<String> {
        let filename_base = self.package_name();
        vec![
            encode_path(&format!("package-2/{}.zip", filename_base)),
            format!("package-1/{}.zip", filename_base),
        ]
    }
}

impl std::fmt::Display for PackageUri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_display_string())
    }
}

impl PartialEq for PackageUri {
    fn eq(&self, other: &Self) -> bool {
        self.authority == other.authority && self.path == other.path
    }
}

impl Eq for PackageUri {}

impl Serialize for PackageUri {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_display_string())
    }
}

impl<'de> Deserialize<'de> for PackageUri {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let input = String::deserialize(deserializer)?;
        PackageUri::create(&input).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Checksums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checksums {
    pub sha256: String,
}

// ---------------------------------------------------------------------------
// Resolved dependencies (PklProject.deps.json entries)
// ---------------------------------------------------------------------------

/// One entry of `resolvedDependencies` in `PklProject.deps.json`.
///
/// Tagged JSON: `{"type": "local" | "remote", ...}` (`@SerialName` upstream).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ResolvedDependency {
    Local(LocalDependency),
    Remote(RemoteDependency),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalDependency {
    pub uri: PackageUri,
    /// Project-relative path to the local project directory.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteDependency {
    pub uri: PackageUri,
    pub checksums: Option<Checksums>,
}

// ---------------------------------------------------------------------------
// Package metadata (cached alongside the ZIP)
// ---------------------------------------------------------------------------

/// JSON metadata stored in the Pkl package cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageMetadata {
    pub dependencies: BTreeMap<String, RemoteDependency>,
    pub name: String,
    pub package_zip_url: String,
    pub version: String,
    pub package_uri: PackageUri,
}

// ---------------------------------------------------------------------------
// PklProject
// ---------------------------------------------------------------------------

/// Metadata the CLI derives from a `PklProject` file — this is the payload of
/// the JSON documents under `.externalPackages/`/project metadata dirs, not
/// the `PklProject` Pkl source itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DerivedProjectMetadata {
    pub project_file_uri: String,
    #[serde(default)]
    pub package_uri: Option<PackageUri>,
    #[serde(default)]
    pub declared_dependencies: BTreeMap<String, PackageUri>,
    #[serde(default)]
    pub evaluator_settings: Option<EvaluatorSettings>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluatorSettings {
    #[serde(default)]
    pub module_cache_dir: Option<String>,
    #[serde(default)]
    pub module_path: Option<Vec<String>>,
}
// ---------------------------------------------------------------------------
// Dependencies
// ---------------------------------------------------------------------------

/// Either a package dependency or a local project reference.
#[derive(Debug, Clone, PartialEq)]
pub enum Dependency {
    /// A remote (or cacheable) package.
    Package(PackageDependency),
    /// A dependency pointing at another project in the workspace.
    LocalProject(LocalProjectDependency),
}

impl Dependency {
    pub fn package_uri(&self) -> &PackageUri {
        match self {
            Dependency::Package(d) => &d.uri,
            Dependency::LocalProject(d) => &d.uri,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackageDependency {
    pub uri: PackageUri,
    pub checksums: Option<Checksums>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalProjectDependency {
    pub uri: PackageUri,
    pub project_dir: PathBuf,
}

/// Convert a resolved-deps entry into a [`Dependency`], resolving local paths
/// against the referencing project's directory.
pub fn resolved_to_dependency(resolved: &ResolvedDependency, project_dir: &Path) -> Dependency {
    match resolved {
        ResolvedDependency::Local(LocalDependency { uri, path }) => {
            Dependency::LocalProject(LocalProjectDependency {
                uri: uri.to_owned(),
                project_dir: project_dir.join(path),
            })
        }
        ResolvedDependency::Remote(RemoteDependency { uri, checksums }) => {
            Dependency::Package(PackageDependency {
                uri: uri.to_owned(),
                checksums: checksums.clone(),
            })
        }
    }
}

/// The `schemaVersion` + `resolvedDependencies` document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDeps {
    pub schema_version: i32,
    pub resolved_dependencies: BTreeMap<String, ResolvedDependency>,
}

impl ProjectDeps {
    /// Find the resolved dependency whose key is a prefix of `package_uri`.
    pub fn get_resolved_dependency(&self, package_uri: &str) -> Option<&ResolvedDependency> {
        self.resolved_dependencies
            .iter()
            .find(|&(key, _)| package_uri.starts_with(key))
            .map(|(_, val)| val)
    }
}

/// A discovered Pkl project: its derived metadata plus (once synced) its
/// resolved dependency graph.
#[derive(Debug, Clone, PartialEq)]
pub struct PklProject {
    pub metadata: DerivedProjectMetadata,
    /// `None` until `pkl project resolve` has run and produced a
    /// `PklProject.deps.json`.
    pub project_deps: Option<ProjectDeps>,
}

impl PklProject {
    pub fn parse_metadata(input: &str) -> Result<Vec<Self>, serde_json::Error> {
        let derived_metadata = serde_json::from_str::<Vec<DerivedProjectMetadata>>(input)?;

        Ok(derived_metadata
            .into_iter()
            .map(|metadata| PklProject {
                metadata,
                project_deps: None,
            })
            .collect())
    }

    pub fn load_project_deps(input: &str) -> Result<ProjectDeps, serde_json::Error> {
        serde_json::from_str(input)
    }

    /// Directory containing the `.pkl-project` file.
    pub fn project_dir(&self) -> Option<PathBuf> {
        let project_file_uri = Uri::parse(self.metadata.project_file_uri.clone()).ok()?;
        let file_path = url_to_path(&project_file_uri).ok()?;
        file_path.parent().map(Path::to_path_buf)
    }
}

// ---------------------------------------------------------------------------
// Workspace state persistence
// ---------------------------------------------------------------------------

/// What gets persisted so projects survive server restarts without re-running
/// `pkl project resolve`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PklWorkspaceState {
    pub schema_version: i32,
    pub pkl_projects: Vec<PklWorkspaceStateEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PklWorkspaceStateEntry {
    pub project_file: String,
    pub package_uri: Option<PackageUri>,
    pub declared_dependencies: BTreeMap<String, PackageUri>,
    pub resolved_dependencies: Option<BTreeMap<String, ResolvedDependency>>,
    pub evaluator_settings: Option<EvaluatorSettings>,
}
