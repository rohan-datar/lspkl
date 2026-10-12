use crate::{
    FileError,
    uri::{archive_parts, archive_uri, normalize_member},
    url_to_path,
};
use fluent_uri::Uri;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub(crate) fn io(path: &Path, source: std::io::Error) -> FileError {
    FileError::Io {
        path: path.to_owned(),
        source,
    }
}

pub(crate) fn archive_contents(uri: &Uri<String>) -> Result<String, FileError> {
    let (archive, member) = archive_parts(uri)?;
    let path = url_to_path(&archive)?;
    let mut zip = zip::ZipArchive::new(fs::File::open(&path).map_err(|e| io(&path, e))?)?;
    let mut file = zip.by_name(&normalize_member(&member)?)?;
    if file.is_dir() || file.is_symlink() {
        return Err(FileError::InvalidSource(uri.to_string()));
    }
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|e| io(&path, e))?;
    Ok(contents)
}

pub(crate) fn list(root: &Uri<String>) -> Result<Vec<Uri<String>>, FileError> {
    let mut result = Vec::new();
    match root.scheme().as_str() {
        "jar" => {
            let (archive, prefix) = archive_parts(root)?;
            let prefix = normalize_member(&prefix)?;
            let path = url_to_path(&archive)?;
            let mut zip = zip::ZipArchive::new(fs::File::open(&path).map_err(|e| io(&path, e))?)?;
            let prefix = if prefix.is_empty() {
                prefix
            } else {
                format!("{}/", prefix.trim_end_matches('/'))
            };
            for index in 0..zip.len() {
                let entry = zip.by_index(index)?;
                let name = entry.name();
                if entry.is_dir() || entry.is_symlink() || !name.starts_with(&prefix) {
                    continue;
                }
                // ZIP entries are untrusted. Never expose absolute or non-canonical paths.
                if name.starts_with('/')
                    || !matches!(normalize_member(name), Ok(ref normalized) if normalized == name)
                {
                    continue;
                }
                result.push(archive_uri(&archive, name)?);
            }
        }
        "file" => {
            let path = url_to_path(root)?;
            if path.is_file() {
                return Ok(vec![root.clone()]);
            }
            let mut pending = vec![path];
            while let Some(dir) = pending.pop() {
                for entry in fs::read_dir(&dir).map_err(|e| io(&dir, e))? {
                    let entry = entry.map_err(|e| io(&dir, e))?;
                    let path = entry.path();
                    let kind = entry.file_type().map_err(|e| io(&path, e))?;
                    if kind.is_dir() {
                        pending.push(path);
                    } else if kind.is_file() {
                        result.push(crate::path_to_uri(&path)?);
                    }
                }
            }
        }
        scheme => return Err(FileError::UnsupportedScheme(scheme.to_owned())),
    }
    result.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    result.dedup();
    Ok(result)
}

pub(crate) fn https_contents(
    uri: &Uri<String>,
    client: &reqwest::blocking::Client,
    cache: &Path,
) -> Result<String, FileError> {
    let parsed =
        url::Url::parse(uri.as_str()).map_err(|_| FileError::InvalidSource(uri.to_string()))?;
    if !parsed.username().is_empty() || parsed.password().is_some() || parsed.fragment().is_some() {
        return Err(FileError::InvalidSource(uri.to_string()));
    }
    // Hash the entire canonical URI: host, port, path and query all participate.
    let key = format!("{:x}", Sha256::digest(uri.as_str().as_bytes()));
    let path = cache.join("https").join(format!("{key}.pkl"));
    match fs::read_to_string(&path) {
        Ok(contents) => return Ok(contents),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io(&path, e)),
    }
    let mut response = client.get(uri.as_str()).send()?.error_for_status()?;
    if response.url().scheme() != "https" {
        return Err(FileError::UnsupportedScheme(
            response.url().scheme().to_owned(),
        ));
    }
    let mut contents = String::new();
    response
        .read_to_string(&mut contents)
        .map_err(|e| io(&path, e))?;
    let parent = path.parent().expect("cache entry has parent");
    fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| io(parent, e))?;
    temporary
        .write_all(contents.as_bytes())
        .map_err(|e| io(&path, e))?;
    temporary.as_file().sync_all().map_err(|e| io(&path, e))?;
    temporary.persist(&path).map_err(|e| io(&path, e.error))?;
    Ok(contents)
}

pub fn default_cache() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("lspkl/modules")
}
