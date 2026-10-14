//! Version-gated language features.

use std::sync::LazyLock;

use lspkl_db::File;
use lspkl_syntax::ast::{NodeKind, TokenType, nodes};
use lspkl_types::{AnalysisContext, NodeRef};
use semver::Version;

const DEFAULT_PKL_VERSION: Version = Version::new(0, 31, 0);
static MODULE_INFO_ANNOTATION: &str = "ModuleInfo";
static MIN_VERSION_ATTR: &str = "minPklVersion";

/// One version-gated feature.
pub struct Feature {
    /// Short human-readable name, for diagnostics.
    pub name: &'static str,

    /// Minimum Pkl version that supports this feature.
    pub required_version: Version,

    /// Error message when the feature is used on an older Pkl version.
    pub message: &'static str,

    /// Returns `true` if `node` is a use site of this feature.
    pub predicate: fn(NodeRef, &AnalysisContext) -> bool,
}

/// Static registry of every version-gated feature.
///
/// Add an entry here as Pkl's grammar adds new constructs that need
/// version-gating. The order is irrelevant — the analyzer iterates and
/// short-circuits on the first match.
pub static FEATURES: LazyLock<Vec<Feature>> = LazyLock::new(|| {
    vec![Feature {
        name: "const object member",
        required_version: Version::new(0, 27, 0),
        message: "Modifier 'const' cannot be applied to object members in this Pkl version.",
        predicate: is_const_on_object_property,
    }]
});

fn is_const_on_object_property(node: NodeRef, ctx: &AnalysisContext) -> bool {
    let Some(tree) = ctx.tree(node.file) else {
        return false;
    };
    let node = node.value;
    if !matches!(tree.kind(node), NodeKind::Terminal(TokenType::Const)) {
        return false;
    }
    let Some(parent) = tree.parent(node) else {
        return false;
    };
    matches!(tree.kind(parent), NodeKind::ObjectProperty)
}

/// The Pkl version a module declares it targets. Used to decide whether
/// version-gated features are supported.
pub fn effective_pkl_version(ctx: &AnalysisContext, file: File) -> Version {
    declared_pkl_version(ctx, file).unwrap_or(DEFAULT_PKL_VERSION)
}

fn declared_pkl_version(ctx: &AnalysisContext, file: File) -> Option<Version> {
    let tree = ctx.tree(file)?;
    let header = nodes::Module::cast(tree, tree.root())?.header(tree)?;
    let annotation = nodes::ModuleHeader::cast(tree, header)?
        .annotations(tree)
        .find(|&node| {
            nodes::Annotation::cast(tree, node)
                .and_then(|annotation| annotation.name(tree))
                .is_some_and(|name| {
                    tree.qualified_identifier_segments(name).last().copied()
                        == Some(MODULE_INFO_ANNOTATION)
                })
        })?;
    let body = nodes::Annotation::cast(tree, annotation)?.body(tree)?;
    let property = nodes::ObjectBody::cast(tree, body)?
        .members(tree)
        .find(|&node| {
            nodes::ObjectProperty::cast(tree, node).is_some()
                && tree.name(node) == Some(MIN_VERSION_ATTR)
        })?;
    let version_node = nodes::ObjectProperty::cast(tree, property)?.value(tree)?;
    let version_text = tree.escaped_string_literal(version_node)?;
    let padded = match version_text.matches('.').count() {
        1 => format!("{version_text}.0"),
        _ => version_text,
    };
    Version::parse(&padded).ok()
}
