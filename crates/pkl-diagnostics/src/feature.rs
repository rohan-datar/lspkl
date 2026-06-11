//! Version-gated language features.

use std::sync::LazyLock;

use pkl_analysis::ast::{NodeId, NodeKind, TokenType};
use pkl_types::AnalysisContext;
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
    pub predicate: fn(NodeId, &AnalysisContext) -> bool,
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

fn is_const_on_object_property(node: NodeId, ctx: &AnalysisContext) -> bool {
    if !matches!(ctx.tree.kind(node), NodeKind::Terminal(TokenType::Const)) {
        return false;
    }
    let Some(parent) = ctx.tree.parent(node) else {
        return false;
    };
    matches!(ctx.tree.kind(parent), NodeKind::ObjectProperty)
}

/// The Pkl version a module declares it targets. Used to decide whether
/// version-gated features are supported.
pub fn effective_pkl_version(ctx: &AnalysisContext) -> Version {
    declared_pkl_version(ctx).unwrap_or(DEFAULT_PKL_VERSION)
}

fn declared_pkl_version(ctx: &AnalysisContext) -> Option<Version> {
    let module_header = ctx
        .tree
        .child(ctx.tree.root(), |k| matches!(k, NodeKind::ModuleHeader))?;

    let &module_annotation = ctx
        .tree
        .children_matching(module_header, |k| matches!(k, NodeKind::Annotation))
        .iter()
        .find(|&&n| {
            ctx.tree
                .child(n, |c| matches!(c, NodeKind::QualifiedIdentifier))
                .map(|name_node| {
                    ctx.tree
                        .qualified_identifier_segments(name_node)
                        .last()
                        .copied()
                        == Some(MODULE_INFO_ANNOTATION)
                })
                .unwrap_or(false)
        })?;

    let annotation_body = ctx
        .tree
        .child(module_annotation, |k| matches!(k, NodeKind::ObjectBody))?;

    let &min_version_property = ctx
        .tree
        .children_matching(annotation_body, |k| matches!(k, NodeKind::ObjectProperty))
        .iter()
        .find(|&&n| ctx.tree.name(n) == Some(MIN_VERSION_ATTR))?;

    let version_node = ctx
        .tree
        .child(min_version_property, |k| k.is_string_literal())?;

    let version_text = ctx.tree.escaped_string_literal(version_node)?;
    let padded = match version_text.matches('.').count() {
        1 => format!("{version_text}.0"),
        _ => version_text,
    };
    Version::parse(&padded).ok()
}
