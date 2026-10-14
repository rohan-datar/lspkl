//! Native editor URIs and archive-safe resolution.
use crate::FileError;
use fluent_uri::{Uri, UriRef};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

/// Convert protocol/source text once, parsing borrowed input before owning its identity.
pub fn parse_source_uri(value: &str) -> Result<Uri<String>, FileError> {
    let uri = Uri::parse(value)?;
    match uri.scheme().as_str() {
        "jar" | "lspkl" => effective_uri(&uri.to_owned()),
        _ => Ok(uri.normalize()),
    }
}

pub fn effective_uri(uri: &Uri<String>) -> Result<Uri<String>, FileError> {
    if uri.scheme().as_str() != "lspkl" {
        return canonical(uri);
    }
    let host = uri.authority().map(|authority| authority.host());
    let path = uri.path().as_str().trim_start_matches('/');
    let path = path.replace('+', " ");
    let decoded = percent_decode_str(&path)
        .decode_utf8()
        .map_err(|_| FileError::InvalidSource(uri.to_string()))?;
    let value = match host {
        Some("stdlib") => format!("pkl:{}", decoded.trim_end_matches(".pkl")),
        Some("https" | "jar") => decoded.into_owned(),
        _ => return Err(FileError::InvalidSource(uri.to_string())),
    };
    let result = Uri::parse(value)?;
    if (host == Some("https") && result.scheme().as_str() != "https")
        || (host == Some("jar") && result.scheme().as_str() != "jar")
    {
        return Err(FileError::InvalidSource(uri.to_string()));
    }
    canonical(&result)
}

pub fn to_editor_uri(uri: &Uri<String>) -> Result<Uri<String>, FileError> {
    let uri = effective_uri(uri)?;
    let value = match uri.scheme().as_str() {
        "file" => return Ok(uri),
        "pkl" => format!("lspkl://stdlib/{}.pkl", uri.path().as_str()),
        scheme @ ("https" | "jar") => {
            // Form encoding: spaces become '+'; '*' stays literal.
            let encoded = utf8_percent_encode(uri.as_str(), NON_ALPHANUMERIC)
                .to_string()
                .replace("%20", "+")
                .replace("%2A", "*")
                .replace("%2D", "-")
                .replace("%2E", ".")
                .replace("%5F", "_");
            format!("lspkl://{scheme}/{encoded}")
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
        Ok(uri.normalize())
    }
}

/// The physical archive containing a `jar:` source, without decoding or copying its member.
pub fn archive_file_uri(uri: &Uri<String>) -> Result<Uri<String>, FileError> {
    let (archive, _) = uri
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
    Ok(archive)
}

pub(crate) fn archive_parts(uri: &Uri<String>) -> Result<(Uri<String>, String), FileError> {
    let archive = archive_file_uri(uri)?;
    let (_, member) = uri
        .as_str()
        .split_once("!/")
        .ok_or_else(|| FileError::InvalidSource(uri.to_string()))?;
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
    if let Ok(absolute) = Uri::parse(reference) {
        return effective_uri(&absolute.to_owned());
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
    let encoded;
    let reference = match UriRef::parse(reference) {
        Ok(reference) => reference,
        Err(_) => {
            const LITERAL_ESCAPE: percent_encoding::AsciiSet = percent_encoding::CONTROLS
                .add(b' ')
                .add(b'"')
                .add(b'<')
                .add(b'>')
                .add(b'`')
                .add(b'{')
                .add(b'}')
                .add(b'|')
                .add(b'[')
                .add(b']')
                .add(b'^')
                .add(b'\\');
            encoded = utf8_percent_encode(reference, &LITERAL_ESCAPE).to_string();
            UriRef::parse(encoded.as_str())
                .map_err(|_| FileError::InvalidSource(reference.to_owned()))?
        }
    };
    let base = source.strip_fragment();
    let target = reference
        .resolve_against(&base)
        .map_err(|_| FileError::InvalidSource(reference.as_str().to_owned()))?;
    // File references cannot traverse above a Windows drive root. Network
    // references and references naming another drive retain their own roots.
    if source.scheme().as_str().eq_ignore_ascii_case("file")
        && reference.authority().is_none()
        && file_drive(target.path().as_str()).is_none()
        && let Some(drive) = file_drive(source.path().as_str())
    {
        let path_start = target.scheme().as_str().len()
            + 1
            + target
                .authority()
                .map_or(0, |authority| 2 + authority.as_str().len());
        let prefix = [drive.as_bytes()[0], b':', b'/'];
        let prefix = std::str::from_utf8(&prefix).expect("file drive prefixes are ASCII");
        let mut target = target.into_string();
        target.insert_str(path_start + 1, prefix);
        let target = Uri::parse(target).map_err(|(_, value)| FileError::InvalidSource(value))?;
        return canonical(&target);
    }
    canonical(&target)
}

fn file_drive(path: &str) -> Option<&str> {
    let drive = path.as_bytes().get(..3)?;
    (drive[0] == b'/'
        && drive[1].is_ascii_alphabetic()
        && drive[2] == b':'
        && path.as_bytes().get(3).is_none_or(|next| *next == b'/'))
    .then(|| &path[1..3])
}
