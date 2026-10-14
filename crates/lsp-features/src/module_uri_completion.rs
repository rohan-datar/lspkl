//! Source-aware module URI completion and lazy module documentation.
use std::collections::{BTreeMap, BTreeSet};

use crate::positions::{PositionConverter, PositionEncoding};
use fluent_uri::Uri;
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit, Documentation,
    MarkupContent, MarkupKind, TextEdit,
};
use pkl_project::ModuleResolver;
use pkl_syntax::ast::NodeKind;
use pkl_syntax::{TextRange, TextSize};

use pkl_db::{Db, File, FileStatus};

pub fn completion(
    db: &dyn Db,
    file: File,
    offset: TextSize,
    encoding: PositionEncoding,
    resolver: &dyn ModuleResolver,
) -> Option<CompletionResponse> {
    let tree = pkl_db::parse(db, file).as_ref().ok()?;
    let positions = PositionConverter::new(tree, encoding);
    let mut node = tree.find_at(offset)?;
    let offset = usize::from(offset);
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
    let span = std::ops::Range::<usize>::from(tree.span(literal));
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
    let range = positions.range(TextRange::new(
        TextSize::try_from(start).ok()?,
        TextSize::try_from(end).ok()?,
    ));
    let glob = tree.is_glob_import(owner);
    let source = &pkl_vfs::Source {
        url: file.uri(db).clone(),
        text: file.text(db).clone(),
    };
    let project =
        pkl_db::project_for_file(db, file).and_then(|project| project.metadata(db).as_deref());
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
        for name in resolver.dependency_names(source, project) {
            let value = format!("@{name}");
            if matches_prefix(&value, &prefix) {
                add(value.clone(), value, false, None);
            }
        }
    } else if prefix.starts_with("package://") && !prefix.contains('#') {
        let packages = resolver.cached_packages();
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
        for (name, folder, target) in resolver.modulepath_children(parent, project) {
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
        if let Some(root) = resolver.completion_root(&prefix, source, project) {
            let path = decode_uri_path(path)?;
            let (parent, child) = split_path(&path);
            let directory = resolver
                .resolve_uri(&root, &format!("./{}", encode_path(parent)))
                .ok()?;
            for (name, folder, target) in resolver.children(&directory) {
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
            directories = resolver.upward_directories(source, parent);
        } else if !absolute_uri
            && let Some(relative) = resolver.modulepath_relative_path(source, project)
        {
            let source_parent = relative.rsplit_once('/').map_or("", |(p, _)| p);
            let logical_parent = if parent.starts_with('/') {
                parent.trim_start_matches('/').to_owned()
            } else if source_parent.is_empty() {
                parent.to_owned()
            } else {
                format!("{source_parent}/{parent}")
            };
            for (name, folder, target) in resolver.modulepath_children(&logical_parent, project) {
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
            if let Ok(directory) = resolver.resolve_uri(&source.url, &reference) {
                directories.push(directory);
            }
        }
        for directory in directories {
            for (name, folder, target) in resolver.children(&directory) {
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

/// Parse cached/local candidate input only when its documentation is requested.
pub fn resolve(db: &dyn Db, mut item: CompletionItem) -> CompletionItem {
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
    let Ok(file) = db.file(&target) else {
        return item;
    };
    if file.status(db) == FileStatus::Missing {
        return item;
    }
    let Ok(tree) = pkl_db::parse(db, file) else {
        return item;
    };
    let header = tree.child(tree.root(), |kind| matches!(kind, NodeKind::ModuleHeader));
    let clause =
        header.and_then(|header| tree.child(header, |kind| matches!(kind, NodeKind::ModuleClause)));
    let signature = clause
        .map(|clause| tree.text(clause).trim().to_owned())
        .unwrap_or_else(|| format!("module {}", target.as_str()));
    let mut markdown = format!("```pkl\n{signature}\n```");
    if let Some(comments) = header.and_then(|header| tree.doc_comment_text(header)) {
        markdown.push_str("\n\n---\n\n");
        markdown.push_str(&comments);
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
use pkl_vfs::{decode_uri_path, encode_path};
fn encode_reference(path: &str) -> String {
    path.strip_prefix("file:///").map_or_else(
        || encode_path(path),
        |path| format!("file:///{}", encode_path(path)),
    )
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
