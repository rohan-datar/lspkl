//! Kotlin-compatible editor URIs and archive-safe resolution.
use crate::FileError;
use fluent_uri::Uri;
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

pub fn effective_uri(uri: &Uri<String>) -> Result<Uri<String>, FileError> {
    if uri.scheme().as_str() != "pkl-lsp" {
        return canonical(uri);
    }
    let parsed =
        url::Url::parse(uri.as_str()).map_err(|_| FileError::InvalidSource(uri.to_string()))?;
    let path = parsed.path().trim_start_matches('/');
    let path = path.replace('+', " ");
    let decoded = percent_decode_str(&path)
        .decode_utf8()
        .map_err(|_| FileError::InvalidSource(uri.to_string()))?;
    let value = match parsed.host_str() {
        Some("stdlib") => format!("pkl:{}", decoded.trim_end_matches(".pkl")),
        Some("https" | "jar") => decoded.into_owned(),
        _ => return Err(FileError::InvalidSource(uri.to_string())),
    };
    let result = Uri::parse(value)?;
    if (parsed.host_str() == Some("https") && result.scheme().as_str() != "https")
        || (parsed.host_str() == Some("jar") && result.scheme().as_str() != "jar")
    {
        return Err(FileError::InvalidSource(uri.to_string()));
    }
    canonical(&result)
}

pub fn to_editor_uri(uri: &Uri<String>) -> Result<Uri<String>, FileError> {
    let uri = effective_uri(uri)?;
    let value = match uri.scheme().as_str() {
        "file" => return Ok(uri),
        "pkl" => format!("pkl-lsp://stdlib/{}.pkl", uri.path().as_str()),
        scheme @ ("https" | "jar") => {
            // Java URLEncoder uses form encoding: spaces become '+'; '*' stays literal.
            let encoded = utf8_percent_encode(uri.as_str(), NON_ALPHANUMERIC)
                .to_string()
                .replace("%20", "+")
                .replace("%2A", "*")
                .replace("%2D", "-")
                .replace("%2E", ".")
                .replace("%5F", "_");
            format!("pkl-lsp://{scheme}/{encoded}")
        }
        scheme => return Err(FileError::UnsupportedScheme(scheme.to_owned())),
    };
    Ok(Uri::parse(value)?)
}

pub(crate) fn canonical(uri: &Uri<String>) -> Result<Uri<String>, FileError> {
    if uri.scheme().as_str() == "jar" {
        let (archive, member) = archive_parts(uri)?;
        archive_uri(&archive, &normalize_member(&member)?)
    } else {
        Ok(uri.clone().normalize())
    }
}

pub(crate) fn archive_parts(uri: &Uri<String>) -> Result<(Uri<String>, String), FileError> {
    let (archive, member) = uri
        .as_str()
        .strip_prefix("jar:")
        .and_then(|s| s.split_once("!/"))
        .ok_or_else(|| FileError::InvalidSource(uri.to_string()))?;
    let archive = Uri::parse(archive.to_owned())?;
    if archive.scheme().as_str() != "file" {
        return Err(FileError::UnsupportedScheme(
            archive.scheme().as_str().to_owned(),
        ));
    }
    let member = percent_decode_str(member)
        .decode_utf8()
        .map_err(|_| FileError::InvalidSource(uri.to_string()))?
        .into_owned();
    Ok((archive, member))
}

pub(crate) fn normalize_member(member: &str) -> Result<String, FileError> {
    let mut parts = Vec::new();
    for part in member.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(FileError::InvalidSource(member.to_owned()));
                }
            }
            _ if part.contains('\\') || part.contains('\0') => {
                return Err(FileError::InvalidSource(member.to_owned()));
            }
            _ => parts.push(part),
        }
    }
    let mut result = parts.join("/");
    if member.ends_with('/') && !result.is_empty() {
        result.push('/');
    }
    Ok(result)
}

pub(crate) fn archive_uri(archive: &Uri<String>, member: &str) -> Result<Uri<String>, FileError> {
    const SEGMENT: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');
    let encoded = member
        .split('/')
        .map(|s| utf8_percent_encode(s, SEGMENT).to_string())
        .collect::<Vec<_>>()
        .join("/");
    Ok(Uri::parse(format!(
        "jar:{}!/{encoded}",
        archive.clone().normalize()
    ))?)
}

pub(crate) fn resolve(source: &Uri<String>, reference: &str) -> Result<Uri<String>, FileError> {
    let source = effective_uri(source)?;
    if let Ok(absolute) = Uri::parse(reference.to_owned()) {
        return effective_uri(&absolute);
    }
    if source.scheme().as_str() == "jar" {
        let (archive, member) = archive_parts(&source)?;
        let reference = percent_decode_str(reference)
            .decode_utf8()
            .map_err(|_| FileError::InvalidSource(reference.to_owned()))?;
        let joined = if reference.starts_with('/') {
            reference.trim_start_matches('/').to_owned()
        } else {
            let parent = if member.ends_with('/') {
                member.as_str()
            } else {
                member.rsplit_once('/').map_or("", |(p, _)| p)
            };
            format!("{parent}/{reference}")
        };
        return archive_uri(&archive, &normalize_member(&joined)?);
    }
    if source.scheme().as_str() == "pkl" {
        return Err(FileError::InvalidSource(reference.to_owned()));
    }
    let parsed = url::Url::parse(source.as_str())
        .map_err(|_| FileError::InvalidSource(source.to_string()))?;
    let target = parsed
        .join(reference)
        .map_err(|_| FileError::InvalidSource(reference.to_owned()))?;
    effective_uri(&Uri::parse(target.as_str().to_owned())?)
}
