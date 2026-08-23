//! Hover: resolve the node at the cursor, render its type + docs as markdown.

use tower_lsp_server::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};

use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};
use pkl_types::compute_expr::compute_expr_type;
use pkl_types::to_type::to_type;
use pkl_types::{
    AnalysisContext, FileId, NodeRef, Type, TypeBindings, resolver::resolve_reference,
};

/// Resolve hover information for the given cursor `position`.
///
/// Returns `None` when there is nothing meaningful at the cursor — the
/// caller should translate `None` into an empty `Hover` per the LSP spec.
///
/// `position` is LSP-style (0-indexed line/column). `pkl_syntax` spans are
/// 1-indexed, so this function is responsible for the conversion before
/// calling `SyntaxTree::find_at`.
///
/// Pipeline:
///   1. Locate the deepest node at the cursor (`find_at`).
///   2. Resolve what it refers to (`resolve_reference`).
///   3. Render the resolved declaration as Markdown (`render_markdown`).
///   4. Wrap as a Markdown `Hover`.
pub fn hover(ctx: &AnalysisContext, position: Position) -> Option<Hover> {
    let line = position.line + 1;
    let col = position.character + 1;

    let node_id = ctx.tree.find_at(line, col)?;
    let original = NodeRef::new(ctx.file_id, node_id);
    let resolved = resolve_reference(original, line, col, ctx)?;
    let value = render_markdown(resolved, original, ctx)?;

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: None,
    })
}

/// Render a resolved declaration as Markdown.
///
/// Returns `None` for node kinds we don't expose in hover (anything that
/// isn't a declaration the user cares about — bare terminals, punctuation,
/// etc.). The caller turns `None` into "no hover".
///
/// `original` is the node the user actually pointed at; `resolved` is what
/// `resolve_reference` returned. They differ when the cursor was on a
/// reference (`x` in `y = x`) and `resolved` is the declaration site
/// (`x = 1`). A few rendering paths need `original` to know which sub-span
/// to highlight (e.g. type-parameter substitution).
pub fn render_markdown(
    resolved: NodeRef,
    original: NodeRef,
    ctx: &AnalysisContext,
) -> Option<String> {
    // `original` is only needed by the type-parameter-substitution paths
    // (renderTypeAnnotation in toMarkdown.kt:159), which aren't ported yet.
    let _ = original;

    // `resolved` may live in another file (e.g. pkl:base), so look up its
    // tree — never use `ctx.tree` directly here.
    let tree = ctx.tree_for(resolved.file_id)?;
    let node_id = resolved.node_id;

    let signature = match tree.kind(node_id) {
        NodeKind::ClassProperty => {
            let name = tree.name(node_id)?;
            match property_type(tree, resolved, ctx) {
                Some(ty) => format!("{}{name}: {ty}", modifiers_prefix(tree, node_id)),
                None => format!("{}{name}", modifiers_prefix(tree, node_id)),
            }
        }
        NodeKind::Class => format!(
            "{}class {}",
            modifiers_prefix(tree, node_id),
            tree.name(node_id)?
        ),
        NodeKind::TypeAlias => format!(
            "{}typealias {}",
            modifiers_prefix(tree, node_id),
            tree.name(node_id)?
        ),
        NodeKind::ClassMethod | NodeKind::ObjectMethod => format!(
            "{}{}",
            modifiers_prefix(tree, node_id),
            method_signature(tree, node_id, resolved.file_id, ctx)?
        ),
        _ => return None,
    };

    // Mirror `showDocCommentAndModule` (toMarkdown.kt:212): the fenced
    // signature block first, then a `---` rule and the doc comment.
    // (Upstream also appends an enclosing-module line; lspkl has no
    // module-name/URI machinery yet, so that part is omitted.)
    let mut md = format!("```pkl\n{signature}\n```");
    if let Some(docs) = doc_comment_text(tree, node_id) {
        md.push_str("\n\n---\n\n");
        md.push_str(&docs);
    }
    Some(md)
}

/// Render the type of a `ClassProperty` for hover.
///
/// Prefers the explicit annotation (`x: String`), rendered through
/// `to_type` so aliases and generics display the same way diagnostics do.
/// Falls back to inferring the initializer's type (`x = 1` → `Int`) via
/// `compute_expr_type`. Returns `None` when neither yields anything useful.
fn property_type(tree: &SyntaxTree, property: NodeRef, ctx: &AnalysisContext) -> Option<String> {
    let annotated = tree
        .child(property.node_id, |k| matches!(k, NodeKind::TypeAnnotation))
        .and_then(|ann| tree.child(ann, |k| k.is_type_node()));
    if let Some(type_node) = annotated {
        let ty = to_type(
            NodeRef::new(property.file_id, type_node),
            &TypeBindings::new(),
            ctx,
        );
        return Some(ty.render(ctx));
    }

    // No annotation: infer from the initializer.
    if property.file_id == ctx.file_id {
        let expr = tree.child(property.node_id, |k| k.is_expr_node())?;
        let ty = compute_expr_type(expr, &TypeBindings::new(), ctx);
        if ty != Type::Unknown {
            return Some(ty.render(ctx));
        }
    }
    None
}

/// Render a method's full signature: `function name<T>(params): Ret`,
/// mirroring the `PklMethodHeader` arm of `doRenderMarkdown`
/// (toMarkdown.kt:60).
fn method_signature(
    tree: &SyntaxTree,
    node_id: NodeId,
    file_id: FileId,
    ctx: &AnalysisContext,
) -> Option<String> {
    let header = tree.child(node_id, |k| matches!(k, NodeKind::MethodHeader))?;
    let name = tree.name(node_id)?;

    let type_params = tree
        .child(header, |k| matches!(k, NodeKind::TypeParameterList))
        .map(|list| type_parameters_text(tree, list))
        .unwrap_or_default();
    let params = tree
        .child(header, |k| matches!(k, NodeKind::ParameterList))
        .map(|list| parameters_text(tree, list))
        .unwrap_or_else(|| "()".to_string());
    let ret = return_type_text(tree, node_id, header, file_id, ctx);

    Some(format!("function {name}{type_params}{params}: {ret}"))
}

/// Render `<A, B>` from a `TypeParameterList`.
///
/// Variance markers (`in`/`out`) are anonymous grammar tokens the parser
/// does not keep, so only names are rendered.
fn type_parameters_text(tree: &SyntaxTree, list: NodeId) -> String {
    let parts: Vec<&str> = tree
        .children_matching(list, |k| matches!(k, NodeKind::TypeParameter))
        .iter()
        .filter_map(|&tp| tree.name(tp))
        .collect();
    format!("<{}>", parts.join(", "))
}

/// Render `(n: Int, other)` from a `ParameterList`, mirroring the
/// `PklParameterList` arm (each element through the `PklTypedIdentifier`
/// arm; blank identifiers render as `_`).
fn parameters_text(tree: &SyntaxTree, list: NodeId) -> String {
    let params: Vec<String> = tree
        .children_matching(list, |k| {
            matches!(k, NodeKind::TypedIdentifier | NodeKind::BlankIdentifier)
        })
        .iter()
        .map(|&p| match tree.kind(p) {
            NodeKind::BlankIdentifier => "_".to_string(),
            _ => {
                let name = tree.name(p).unwrap_or("_");
                match tree.child(p, |k| matches!(k, NodeKind::TypeAnnotation)) {
                    // Raw source text: `to_type` currently drops explicit
                    // type arguments (`Function1<A, B>` renders as
                    // `Function1`), so the slice is more faithful for
                    // parameters.
                    Some(ann) => format!(
                        "{name}: {}",
                        tree.child(ann, |k| k.is_type_node())
                            .map(|ty| tree.text(ty))
                            .unwrap_or(""),
                    ),
                    None => name.to_string(),
                }
            }
        })
        .collect();
    format!("({})", params.join(", "))
}

/// Render a method's return type. Prefers the declared annotation;
/// without one, infers from the body expression via `compute_expr_type`
/// (as upstream does); gives up with `unknown`.
fn return_type_text(
    tree: &SyntaxTree,
    method_id: NodeId,
    header: NodeId,
    file_id: FileId,
    ctx: &AnalysisContext,
) -> String {
    if let Some(ann) = tree.child(header, |k| matches!(k, NodeKind::TypeAnnotation)) {
        return annotation_text(tree, ann, file_id, ctx);
    }
    // `compute_expr_type` reads `ctx.tree` directly, so it can only run on
    // nodes from the currently-analyzed file.
    if file_id == ctx.file_id
        && let Some(expr) = tree.child(method_id, |k| k.is_expr_node())
    {
        let ty = compute_expr_type(expr, &TypeBindings::new(), ctx);
        if ty != Type::Unknown {
            return ty.render(ctx);
        }
    }
    "unknown".to_string()
}

/// Render a type annotation's inner type semantically (`to_type` +
/// `render`), falling back to the raw source text when the type can't be
/// resolved.
fn annotation_text(
    tree: &SyntaxTree,
    ann: NodeId,
    file_id: FileId,
    ctx: &AnalysisContext,
) -> String {
    match tree.child(ann, |k| k.is_type_node()) {
        Some(ty_node) => {
            let ty = to_type(NodeRef::new(file_id, ty_node), &TypeBindings::new(), ctx);
            if ty != Type::Unknown {
                ty.render(ctx)
            } else {
                tree.text(ty_node).to_string()
            }
        }
        None => String::new(),
    }
}

/// Modifier keywords (`hidden`, `fixed`, ...) as a trailing-space prefix,
/// mirroring `modifiers.render()` in toMarkdown.kt.
fn modifiers_prefix(tree: &SyntaxTree, node_id: NodeId) -> String {
    // For methods the modifiers live on the MethodHeader, matching
    // `SyntaxTree::modifiers`.
    let holder = match tree.kind(node_id) {
        NodeKind::ClassMethod | NodeKind::ObjectMethod => tree
            .child(node_id, |k| matches!(k, NodeKind::MethodHeader))
            .unwrap_or(node_id),
        _ => node_id,
    };
    let mods: Vec<&str> = tree
        .children_matching(
            holder,
            |k| matches!(k, NodeKind::Terminal(tt) if tt.is_modifier()),
        )
        .iter()
        .map(|&m| tree.text(m))
        .collect();
    if mods.is_empty() {
        String::new()
    } else {
        format!("{} ", mods.join(" "))
    }
}

/// The declaration's doc comment with `///` prefixes stripped (one space
/// after each marker is dropped too), mirroring `PklDocComment.contents`.
///
/// Member links inside comments are left as plain text — link resolution
/// is deferred alongside DocCommentMemberLinkCompletionProvider (DESIGN.md §5).
fn doc_comment_text(tree: &SyntaxTree, node_id: NodeId) -> Option<String> {
    let lines: Vec<String> = tree
        .doc_comments(node_id)
        .iter()
        .flat_map(|doc| doc.trim().lines())
        .map(|line| {
            let stripped = line.strip_prefix("///").unwrap_or(line);
            stripped.strip_prefix(' ').unwrap_or(stripped).to_string()
        })
        .collect();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}
