//! Validate module URI syntax and request-pinned import resolution.

use pkl_types::NodeRef;

use fluent_uri::UriRef;
use pkl_syntax::ast::{NodeKind, nodes};
use pkl_types::{AnalysisContext, ImportResolution};

use crate::analyzer::Analyzer;
use crate::diagnostic::QuickFix;
use crate::holder::DiagnosticsHolder;

pub struct ModuleUriAnalyzer;

impl Analyzer for ModuleUriAnalyzer {
    fn analyze_node(
        &self,
        node: NodeRef,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        let file = node.file;
        let Some(source_tree) = ctx.tree(file) else {
            return false;
        };
        let node = node.value;
        if !source_tree.kind(node).is_module_uri_owner() {
            return true;
        }
        let literal = match source_tree.kind(node) {
            NodeKind::Import => {
                nodes::Import::cast(source_tree, node).and_then(|import| import.uri(source_tree))
            }
            NodeKind::ImportGlob => nodes::ImportGlob::cast(source_tree, node)
                .and_then(|import| import.uri(source_tree)),
            NodeKind::ImportExpr => nodes::ImportExpr::cast(source_tree, node)
                .and_then(|import| import.uri(source_tree)),
            NodeKind::ModuleExtendsAmendsClause => {
                nodes::ModuleExtendsAmendsClause::cast(source_tree, node)
                    .and_then(|clause| clause.target(source_tree))
            }
            _ => None,
        };
        let Some(literal) = literal else {
            return true;
        };
        let Some(uri) = source_tree.escaped_string_literal(literal) else {
            return true;
        };
        let span = source_tree.span(literal);
        let glob = source_tree.is_glob_import(node);
        if glob && uri.starts_with("...") {
            holder.add_error(span, "cannot glob triple imports");
            return true;
        }
        let Ok(parsed) = UriRef::parse(uri.as_str()) else {
            holder.add_warning(span, format!("malformed uri: {uri}"));
            return true;
        };
        if glob
            && let Some(scheme) = parsed.scheme()
            && matches!(scheme.as_str(), "pkl" | "http" | "https")
        {
            holder.add_warning(span, format!("scheme {} is not globbable", scheme.as_str()));
            return true;
        }
        if parsed.scheme().is_some_and(|s| s.as_str() == "package")
            && let Some(message) = package_uri_error(&parsed)
        {
            holder.add_warning(span, message);
            return true;
        }
        // Absence means the caller supplied no graph evidence, not resolution failure.
        match ctx.imports.get(&NodeRef::new(file, node)) {
            Some(ImportResolution::Unresolved {
                package_to_download: Some(package),
            }) => {
                holder.add_warning_with_fix(
                    span,
                    format!("missing package sources: {package}"),
                    QuickFix::download_package(package.clone()),
                );
            }
            Some(ImportResolution::Unresolved {
                package_to_download: None,
            }) => {
                if let Some(dependency) = uri.strip_prefix('@') {
                    let name = dependency.split('/').next().unwrap_or(dependency);
                    holder.add_warning(span, format!("cannot resolve dependency import: {name}"));
                } else {
                    holder.add_warning(span, "cannot resolve import");
                }
            }
            Some(ImportResolution::Glob(targets)) if targets.is_empty() => {
                holder.add_warning(span, "glob pattern has no matches");
            }
            Some(ImportResolution::Single(_) | ImportResolution::Glob(_)) | None => {}
        }
        true
    }
}

fn package_uri_error(uri: &UriRef<&str>) -> Option<String> {
    if uri
        .authority()
        .is_none_or(|authority| authority.as_str().is_empty())
    {
        return Some("missing package authority".into());
    }
    let path = uri.path().as_str();
    if path.is_empty() || path == "/" {
        return Some("missing package path".into());
    }
    let Some((base_path, version_and_checksum)) = path.rsplit_once('@') else {
        return Some("missing package version".into());
    };
    if base_path.is_empty() || base_path == "/" {
        return Some("missing package path".into());
    }
    let (version, checksum) = version_and_checksum
        .split_once("::")
        .map_or((version_and_checksum, None), |(v, c)| (v, Some(c)));
    if version.is_empty() {
        return Some("missing package version".into());
    }
    if semver::Version::parse(version).is_err() {
        return Some(format!("invalid semantic version: {version}"));
    }
    if let Some(checksum) = checksum {
        let valid = checksum.split_once(':').is_some_and(|(algorithm, value)| {
            algorithm == "sha256" && !value.is_empty() && !value.contains(':')
        });
        if !valid {
            return Some(format!("invalid package checksum: {checksum}"));
        }
    }
    let Some(fragment) = uri.fragment() else {
        return Some("missing package fragment".into());
    };
    if !fragment.as_str().starts_with('/') {
        return Some("invalid fragment path".into());
    }
    None
}
