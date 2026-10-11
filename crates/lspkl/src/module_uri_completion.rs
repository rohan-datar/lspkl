//! Source-aware module URI completion and lazy module documentation.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use fluent_uri::Uri;
use pkl_project::import_resolver::ImportResolver;
use pkl_project::modulepath::ModulepathResolver;
use pkl_syntax::ast::NodeKind;
use tower_lsp_server::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit, Documentation,
    MarkupContent, MarkupKind, Position, Range, TextEdit,
};

use crate::state::{AnalysisSnapshot, ServerState};

pub fn completion(
    state: &ServerState,
    analysis: &AnalysisSnapshot,
    position: Position,
) -> Option<CompletionResponse> {
    let tree = &analysis.module.tree;
    let offset = byte_offset(tree.source(), position)?;
    let byte_column = tree.source()[..offset].rsplit('\n').next()?.len() as u32 + 1;
    let mut node = tree.find_at(position.line + 1, byte_column)?;
    while !tree.kind(node).is_module_uri_owner() {
        node = tree.parent(node)?;
    }
    let owner = node;
    let literal = tree.child(owner, NodeKind::is_string_literal)?;
    let text = tree.text(literal);
    let hashes = text.bytes().take_while(|&b| b == b'#').count();
    let quotes = if text[hashes..].starts_with("\"\"\"") {
        3
    } else {
        1
    };
    let delimiter = hashes + quotes;
    let span = tree.span(literal).byte_range();
    let start = span.start + delimiter;
    let end = span.end.checked_sub(delimiter)?;
    if offset < start || offset > end || tree.escaped_string_literal(literal).is_none() {
        return None;
    }
    // Decode only the portion before the cursor, using the same literal semantics
    // as import resolution (including hash-delimited escapes).
    let prefix_literal = format!(
        "{}{}{}",
        &text[..delimiter],
        &tree.source()[start..offset],
        &text[text.len() - delimiter..]
    );
    let prefix = pkl_syntax::ast::decode_string_literal(&prefix_literal)?;
    let range = Range::new(
        lsp_position(tree.source(), start),
        lsp_position(tree.source(), end),
    );
    let glob = tree.is_glob_import(owner);
    let source = &analysis.module.file;
    let project = pkl_vfs::url_to_path(&source.url)
        .ok()
        .and_then(|path| state.project_manager.lock().get_pkl_project_shared(&path));
    let settings = state.settings.read().clone();
    let cache: std::borrow::Cow<'_, Path> = settings
        .resolved_cache_dir
        .as_deref()
        .or(state.package_cache_dir.as_deref())
        .map(std::borrow::Cow::Borrowed)
        .unwrap_or_else(|| std::borrow::Cow::Owned(state.file_manager.cache_dir()));
    let resolver =
        ImportResolver::with_settings(&state.file_manager, &settings.module_paths, &cache);
    let modulepath =
        ModulepathResolver::with_modulepaths(&state.file_manager, &settings.module_paths);
    let mut items = BTreeMap::new();
    let origin = source.url.as_str();
    let mut add = |label: String, value: String, folder: bool, target: Option<Uri<String>>| {
        let item = CompletionItem {
            label: label.clone(),
            kind: Some(if folder { CompletionItemKind::FOLDER } else { CompletionItemKind::MODULE }),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text: escape_literal(&value, hashes) })),
            data: target.map(|uri| serde_json::json!({"moduleUriCompletion": true, "targetUri": uri.as_str(), "originUri": origin, "originRange": range})),
            ..Default::default()
        };
        items.entry(label).or_insert(item);
    };
    if prefix.starts_with("pkl:") {
        if !glob {
            for name in pkl_vfs::stdlib::MODULE_NAMES {
                let value = format!("pkl:{name}");
                if matches_prefix(&value, &prefix) {
                    add(value.clone(), value.clone(), false, Uri::parse(value).ok());
                }
            }
        }
    } else if prefix.starts_with('@') && !prefix.contains('/') {
        for name in resolver.dependency_names(source, project.as_deref()) {
            let value = format!("@{name}");
            if matches_prefix(&value, &prefix) {
                add(value.clone(), value, false, None);
            }
        }
    } else if prefix.starts_with("package://") && !prefix.contains('#') {
        let packages = cached_packages(&cache);
        if let Some((base, version_prefix)) = prefix.rsplit_once('@') {
            for package in packages {
                if let Some((candidate_base, version)) = package.rsplit_once('@')
                    && candidate_base == base
                    && version.starts_with(version_prefix)
                {
                    add(version.to_owned(), format!("{package}#/"), false, None);
                }
            }
        } else {
            for base in packages
                .iter()
                .filter_map(|p| p.rsplit_once('@').map(|(base, _)| base))
                .collect::<BTreeSet<_>>()
            {
                if matches_prefix(base, &prefix) {
                    add(base.to_owned(), format!("{base}@"), false, None);
                }
            }
        }
    } else if prefix.starts_with("modulepath:/") {
        let logical = decode_uri_path(prefix.strip_prefix("modulepath:/")?)?;
        let (parent, child) = split_path(&logical);
        for (name, folder, target) in
            modulepath_children(state, &modulepath, parent, project.as_deref())
        {
            if visible(&name, folder) && matches_prefix(&name, child) {
                let value = format!(
                    "modulepath:/{}{}",
                    encode_path(parent),
                    encode_path(&glob_escape(&name, glob))
                );
                add(name, value, folder, Some(target));
            }
        }
    } else if prefix.starts_with('@') || prefix.starts_with("package://") {
        let (base, path) = if prefix.starts_with('@') {
            let slash = prefix.find('/')?;
            (&prefix[..slash + 1], &prefix[slash + 1..])
        } else {
            let fragment = prefix.find('#')?;
            (
                &prefix[..fragment + 1],
                prefix[fragment + 1..].trim_start_matches('/'),
            )
        };
        if let Some(root) = resolver.completion_root(&prefix, source, project.as_deref()) {
            let path = decode_uri_path(path)?;
            let (parent, child) = split_path(&path);
            let directory = state
                .file_manager
                .resolve_uri(&root, &format!("./{}", encode_path(parent)))
                .ok()?;
            for (name, folder, target) in children(state, &directory) {
                if visible(&name, folder) && matches_prefix(&name, child) {
                    let slash = if base.ends_with('#') { "/" } else { "" };
                    add(
                        name.clone(),
                        format!(
                            "{base}{slash}{}{}",
                            encode_path(parent),
                            encode_path(&glob_escape(&name, glob))
                        ),
                        folder,
                        Some(target),
                    );
                }
            }
        }
    } else if !prefix.contains(':') || prefix.starts_with("file:///") {
        let absolute_uri = prefix.starts_with("file:///");
        if !prefix.contains('/') && !prefix.contains(':') {
            let schemes: &[&str] = if glob {
                &["file:///", "package://"]
            } else {
                &["pkl:", "file:///", "https://", "package://", "modulepath:/"]
            };
            for scheme in schemes {
                if matches_prefix(scheme, &prefix) {
                    add((*scheme).into(), (*scheme).into(), false, None);
                }
            }
        }
        let path = decode_uri_path(&prefix)?;
        let (parent, child) = split_path(&path);
        let upward = path == "..." || path.starts_with(".../");
        if upward && glob {
            return Some(CompletionResponse::Array(items.into_values().collect()));
        }
        let mut directories = Vec::new();
        if upward {
            if let Ok(source_path) = pkl_vfs::url_to_path(&source.url) {
                let suffix = parent.strip_prefix(".../").unwrap_or("");
                for ancestor in source_path.parent()?.ancestors().skip(1) {
                    if let Some(uri) = file_uri(&ancestor.join(suffix), true) {
                        directories.push(uri);
                    }
                }
            }
        } else if !absolute_uri
            && let Some(relative) = modulepath.relative_path(source, project.as_deref())
        {
            let source_parent = relative.rsplit_once('/').map_or("", |(p, _)| p);
            let logical_parent = if parent.starts_with('/') {
                parent.trim_start_matches('/').to_owned()
            } else if source_parent.is_empty() {
                parent.to_owned()
            } else {
                format!("{source_parent}/{parent}")
            };
            for (name, folder, target) in
                modulepath_children(state, &modulepath, &logical_parent, project.as_deref())
            {
                if visible(&name, folder) && matches_prefix(&name, child) {
                    add(
                        name.clone(),
                        format!(
                            "{}{}",
                            encode_path(parent),
                            encode_path(&glob_escape(&name, glob))
                        ),
                        folder,
                        Some(target),
                    );
                }
            }
        } else {
            let reference = if parent.is_empty() {
                "./".to_owned()
            } else {
                encode_reference(parent)
            };
            if let Ok(directory) = state.file_manager.resolve_uri(&source.url, &reference) {
                directories.push(directory);
            }
        }
        for directory in directories {
            for (name, folder, target) in children(state, &directory) {
                let child_match = if path == "..." { "" } else { child };
                if visible(&name, folder) && matches_prefix(&name, child_match) {
                    let parent = if path == "..." { ".../" } else { parent };
                    let guard = if parent.is_empty() && name.starts_with('@') {
                        "./"
                    } else {
                        ""
                    };
                    let component = glob_escape(&name, glob);
                    let component = encode_path(&component);
                    add(
                        name,
                        format!("{guard}{}{component}", encode_reference(parent)),
                        folder,
                        Some(target),
                    );
                }
            }
        }
    }
    Some(CompletionResponse::Array(items.into_values().collect()))
}

/// Fetch and parse the candidate only when the client requests its documentation.
pub fn resolve(state: &ServerState, mut item: CompletionItem) -> CompletionItem {
    let Some(data) = item
        .data
        .as_ref()
        .filter(|d| d.get("moduleUriCompletion").and_then(|v| v.as_bool()) == Some(true))
    else {
        return item;
    };
    let Some(target) = data
        .get("targetUri")
        .and_then(|v| v.as_str())
        .and_then(|s| Uri::parse(s.to_owned()).ok())
    else {
        return item;
    };
    let Ok(file) = state.file_manager.get_or_load(&target) else {
        return item;
    };
    let Ok(module) = state.parse_module(file) else {
        return item;
    };
    let tree = &module.tree;
    let header = tree.child(tree.root(), |kind| matches!(kind, NodeKind::ModuleHeader));
    let clause =
        header.and_then(|header| tree.child(header, |kind| matches!(kind, NodeKind::ModuleClause)));
    let signature = clause
        .map(|clause| tree.text(clause).trim().to_owned())
        .unwrap_or_else(|| format!("module {}", target.as_str()));
    let mut markdown = format!("```pkl\n{signature}\n```");
    let comments = header
        .map(|header| tree.doc_comments(header))
        .unwrap_or_default();
    if !comments.is_empty() {
        markdown.push_str("\n\n---\n\n");
        markdown.push_str(
            &comments
                .iter()
                .flat_map(|comment| comment.lines())
                .map(|line| {
                    let line = line.trim_start().strip_prefix("///").unwrap_or(line);
                    line.strip_prefix(' ').unwrap_or(line)
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    item.detail = Some(signature);
    item.documentation = Some(Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: markdown,
    }));
    item
}

fn split_path(path: &str) -> (&str, &str) {
    path.rfind('/')
        .map_or(("", path), |index| (&path[..index + 1], &path[index + 1..]))
}
fn matches_prefix(value: &str, prefix: &str) -> bool {
    let mut value = value.chars().flat_map(char::to_lowercase);
    prefix
        .chars()
        .flat_map(char::to_lowercase)
        .all(|ch| value.next() == Some(ch))
}
fn visible(name: &str, folder: bool) -> bool {
    !name.starts_with('.')
        && (folder || name.ends_with(".pkl") || name.ends_with(".pcf") || name == "PklProject")
}
fn logical_children(
    files: BTreeMap<String, Uri<String>>,
    parent: &str,
) -> Vec<(String, bool, Uri<String>)> {
    let mut children = BTreeMap::new();
    for (path, uri) in files {
        let Some(rest) = path.strip_prefix(parent) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        let (name, folder) = rest.find('/').map_or((rest.to_owned(), false), |i| {
            (rest[..i + 1].to_owned(), true)
        });
        let target = if folder {
            let encoded = encode_path(&rest[name.len()..]);
            Uri::parse(
                uri.as_str()
                    .strip_suffix(&encoded)
                    .unwrap_or(uri.as_str())
                    .to_owned(),
            )
            .unwrap_or(uri)
        } else {
            uri
        };
        children.entry(name).or_insert((folder, target));
    }
    children
        .into_iter()
        .map(|(name, (folder, uri))| (name, folder, uri))
        .collect()
}
fn children(state: &ServerState, directory: &Uri<String>) -> Vec<(String, bool, Uri<String>)> {
    if let Ok(path) = pkl_vfs::url_to_path(directory) {
        return std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let folder = entry.file_type().ok()?.is_dir();
                let mut name = entry.file_name().into_string().ok()?;
                if folder {
                    name.push('/');
                }
                Some((name, folder, file_uri(&entry.path(), folder)?))
            })
            .collect();
    }
    let mut files = BTreeMap::new();
    if let Ok(uris) = state.file_manager.list_files(directory) {
        for uri in uris {
            if let Some(relative) = uri
                .as_str()
                .strip_prefix(directory.as_str())
                .and_then(decode_uri_path)
            {
                files.insert(relative, uri);
            }
        }
    }
    logical_children(files, "")
}
fn file_uri(path: &Path, directory: bool) -> Option<Uri<String>> {
    let uri = pkl_vfs::path_to_uri(path).ok()?;
    if directory && !uri.as_str().ends_with('/') {
        let mut value = uri.into_string();
        value.push('/');
        Uri::parse(value).ok()
    } else {
        Some(uri)
    }
}
fn encode_path(path: &str) -> String {
    use std::fmt::Write;
    let mut result = String::new();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            result.push(byte as char);
        } else {
            write!(result, "%{byte:02X}").unwrap();
        }
    }
    result
}
fn encode_reference(path: &str) -> String {
    path.strip_prefix("file:///").map_or_else(
        || encode_path(path),
        |path| format!("file:///{}", encode_path(path)),
    )
}
fn decode_uri_path(path: &str) -> Option<String> {
    let mut result = Vec::new();
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let hi = (bytes.next()? as char).to_digit(16)?;
            let lo = (bytes.next()? as char).to_digit(16)?;
            result.push((hi * 16 + lo) as u8);
        } else {
            result.push(byte);
        }
    }
    String::from_utf8(result).ok()
}
fn glob_escape(name: &str, glob: bool) -> String {
    let mut result = String::new();
    for ch in name.chars() {
        if glob && matches!(ch, '*' | '\\' | '{' | '[') {
            result.push('\\');
        }
        result.push(ch);
    }
    result
}
fn escape_literal(value: &str, hashes: usize) -> String {
    let escape = format!("\\{}", "#".repeat(hashes));
    let mut result = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => {
                result.push_str(&escape);
                result.push('\\');
            }
            '"' => {
                result.push_str(&escape);
                result.push('"');
            }
            '\n' => {
                result.push_str(&escape);
                result.push('n');
            }
            '\r' => {
                result.push_str(&escape);
                result.push('r');
            }
            '\t' => {
                result.push_str(&escape);
                result.push('t');
            }
            _ => result.push(ch),
        }
    }
    result
}
fn byte_offset(source: &str, position: Position) -> Option<usize> {
    let mut start = 0;
    for _ in 0..position.line {
        start += source[start..].find('\n')? + 1;
    }
    let mut units = 0;
    for (index, ch) in source[start..].char_indices() {
        if units == position.character {
            return Some(start + index);
        }
        if ch == '\n' {
            return None;
        }
        units += ch.len_utf16() as u32;
    }
    (units == position.character).then_some(source.len())
}
fn lsp_position(source: &str, offset: usize) -> Position {
    let prefix = &source[..offset];
    Position::new(
        prefix.bytes().filter(|&b| b == b'\n').count() as u32,
        prefix
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .encode_utf16()
            .count() as u32,
    )
}
fn cached_packages(cache: &Path) -> BTreeSet<String> {
    let mut packages = BTreeSet::new();
    for layout in ["package-1", "package-2"] {
        let root = cache.join(layout);
        let mut pending = vec![root.clone()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    pending.push(entry.path());
                } else if entry.path().extension().is_some_and(|e| e == "json") {
                    let path = entry.path();
                    let Some(relative) = path
                        .parent()
                        .and_then(|dir| dir.strip_prefix(&root).ok())
                        .and_then(Path::to_str)
                    else {
                        continue;
                    };
                    // Cache paths are native filesystem paths; package URIs use `/`.
                    #[cfg(windows)]
                    let normalized = relative.replace('\\', "/");
                    #[cfg(windows)]
                    let relative = normalized.as_str();
                    let Some(relative) = decode_cache_path(relative, layout == "package-2") else {
                        continue;
                    };
                    let value = format!("package://{relative}");
                    if pkl_project::dto::PackageUri::create(&value).is_ok() {
                        packages.insert(value);
                    }
                }
            }
        }
    }
    packages
}
fn decode_cache_path(path: &str, encoded: bool) -> Option<String> {
    if !encoded {
        return Some(path.to_owned());
    }
    let mut value = String::new();
    let mut rest = path;
    while let Some(index) = rest.find('(') {
        value.push_str(&rest[..index]);
        rest = &rest[index + 1..];
        if let Some(next) = rest.strip_prefix('(') {
            value.push('(');
            rest = next;
        } else {
            let end = rest.find(')')?;
            let code = u32::from_str_radix(&rest[..end], 16).ok()?;
            value.push(char::from_u32(code)?);
            rest = &rest[end + 1..];
        }
    }
    value.push_str(rest);
    Some(value)
}
fn modulepath_children(
    state: &ServerState,
    modulepath: &ModulepathResolver<'_>,
    parent: &str,
    project: Option<&pkl_project::PklProject>,
) -> Vec<(String, bool, Uri<String>)> {
    let mut entries = BTreeMap::new();
    for root in modulepath.roots(project) {
        let Ok(directory) = state
            .file_manager
            .resolve_uri(&root, &format!("./{}", encode_path(parent)))
        else {
            continue;
        };
        for (name, folder, target) in children(state, &directory) {
            entries.entry(name).or_insert((folder, target));
        }
    }
    entries
        .into_iter()
        .map(|(name, (folder, target))| (name, folder, target))
        .collect()
}
