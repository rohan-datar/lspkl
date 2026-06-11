//! Validates `import` and `amends` URIs

use fluent_uri::UriRef;
use pkl_analysis::ast::{NodeId, NodeKind};
use pkl_types::AnalysisContext;

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;

pub struct ModuleUriAnalyzer;

impl Analyzer for ModuleUriAnalyzer {
    fn analyze_node(
        &self,
        node: NodeId,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        if !ctx.tree.kind(node).is_module_uri_owner() {
            return true;
        }
        let Some(uri_node) = ctx
            .tree
            .child(node, |c| matches!(c, NodeKind::StringConstant))
        else {
            return true;
        };
        let Some(uri) = ctx.tree.escaped_string_literal(uri_node) else {
            return true;
        };
        if ctx.tree.is_glob_import(uri_node) {
            Self::analyze_glob_uri(&uri, uri_node, holder, ctx);
            return true;
        }
        // TODO resolve uri and check dependency notation

        if uri.starts_with("package:") && Self::analyze_package_uri(&uri, uri_node, holder, ctx) {
            return true;
        }
        if uri.starts_with("modulepath:") && Self::analyze_package_uri(&uri, uri_node, holder, ctx)
        {
            return true;
        }

        true
    }
}

impl ModuleUriAnalyzer {
    fn analyze_glob_uri(
        uri: &str,
        uri_node: NodeId,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) {
        let uri_span = ctx.tree.span(uri_node);
        if uri.starts_with("...") {
            holder.add_error(uri_span, "cannot glob triple imports");
            return;
        }

        let Ok(parsed_uri) = UriRef::parse(uri) else {
            holder.add_error(uri_span, format!("invalid uri: {uri}"));
            return;
        };
        if let Some(scheme) = parsed_uri.scheme() {
            let scheme = scheme.as_str();
            if scheme == "pkl" || scheme == "http" || scheme == "https" {
                holder.add_warning(uri_span, "scheme is not globbable");
                return;
            }
        }

        // TODO resolve the glob uri
    }

    fn analyze_package_uri(
        uri: &str,
        uri_node: NodeId,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        let uri_span = ctx.tree.span(uri_node);
        let Ok(parsed_uri) = UriRef::parse(uri) else {
            holder.add_warning(uri_span, "malformed uri: {uri}");

            return true;
        };

        if !parsed_uri.has_authority() {
            holder.add_warning(uri_span, "missing package authority");
            return true;
        }

        if parsed_uri.path().is_empty() {
            holder.add_warning(uri_span, "missing package path");
            return true;
        }

        // TODO complete version and checksum checks
        // let version_and_checksum_part = parsed_uri
        //     .path()
        //     .as_str()
        //     .rsplit_once('@')
        //     .map(|(_, tail)| tail)
        //     .unwrap_or("");

        // let version_and_checksum_parts = version_and_checksum_part.split("::");

        if !parsed_uri.has_fragment() {
            holder.add_warning(uri_span, "missing package fragment");
            return true;
        }

        if !parsed_uri.fragment().unwrap().as_str().starts_with("/") {
            holder.add_warning(uri_span, "invalid fragment path");
            return true;
        }

        // TODO check package from package manager

        false
    }
}
