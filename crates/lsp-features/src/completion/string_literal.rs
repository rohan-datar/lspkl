//! String-literal type completion: suggest allowed literal values.

use tower_lsp_server::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionTextEdit,
    Documentation, MarkupContent, MarkupKind, Range, TextEdit,
};

use pkl_syntax::ast::{NodeId, NodeKind};
use pkl_types::infer_expr_type::infer_expr_type_from_context;
use pkl_types::{AnalysisContext, NodeRef, Type, TypeBindings};

pub(crate) fn completions(node: NodeId, ctx: &AnalysisContext) -> Vec<CompletionItem> {
    let Some(literal) = find_string_literal(node, ctx) else {
        return Vec::new();
    };

    // Preferred path: the shared context inference (handles object-entry
    // value positions, argument positions, etc.).
    let expected = infer_expr_type_from_context(literal, &TypeBindings::new(), ctx);
    let mut out = Vec::new();
    let detailed = render_detail(&expected, ctx);
    expand(&expected, None, &detailed, literal, ctx, &mut out);
    if !out.is_empty() {
        return out;
    }

    // Routed-around path: expand the enclosing member's own type annotation,
    // handling `StringLiteralType` explicitly.
    let Some(annotation) = enclosing_type_annotation(literal, ctx) else {
        return out;
    };
    let detail_text = ctx.tree.text(annotation).to_string();
    let mut values = Vec::new();
    annotation_literal_values(annotation, ctx, &mut values, 0);
    for value in values {
        out.push(literal_item(&value, &detail_text, literal, ctx));
    }
    out
}

fn find_string_literal(node: NodeId, ctx: &AnalysisContext) -> Option<NodeId> {
    let tree = ctx.tree;
    if matches!(tree.kind(node), NodeKind::SingleLineStringLiteral) {
        return Some(node);
    }
    let ancestor = tree.ancestor(node, |k| {
        matches!(
            k,
            NodeKind::SingleLineStringLiteral
                | NodeKind::ObjectBody
                | NodeKind::MultiLineStringLiteral
        )
    })?;
    matches!(tree.kind(ancestor), NodeKind::SingleLineStringLiteral).then_some(ancestor)
}

// ---------------------------------------------------------------------------
// Expansion of an inferred [`Type`]
// ---------------------------------------------------------------------------

fn expand(
    result_type: &Type,
    original_alias: Option<&Type>,
    detailed: &str,
    literal: NodeId,
    ctx: &AnalysisContext,
    out: &mut Vec<CompletionItem>,
) {
    match result_type {
        Type::StringLiteral { value, .. } => {
            out.push(literal_item_with_alias(
                value,
                detailed,
                original_alias.map(|t| t.render(ctx)),
                literal,
                ctx,
            ));
        }
        Type::Union { left, right, .. } => {
            expand(left, original_alias, detailed, literal, ctx, out);
            expand(right, original_alias, detailed, literal, ctx, out);
        }
        Type::Alias { .. } => expand(
            &result_type.unaliased(ctx),
            Some(result_type),
            detailed,
            literal,
            ctx,
            out,
        ),
        _ => {}
    }
}

fn render_detail(ty: &Type, ctx: &AnalysisContext) -> String {
    match ty {
        Type::Unknown => String::new(),
        other => other.render(ctx),
    }
}

/// The declared type of the nearest enclosing member (property/method),
/// i.e. the type node inside its `TypeAnnotation`.
fn enclosing_type_annotation(start: NodeId, ctx: &AnalysisContext) -> Option<NodeId> {
    super::ancestor_or_self(start, ctx, |k| {
        matches!(
            k,
            NodeKind::ClassProperty
                | NodeKind::ObjectProperty
                | NodeKind::ClassMethod
                | NodeKind::ObjectMethod
        )
    })
    .and_then(|member| {
        let holder = match ctx.tree.kind(member) {
            NodeKind::ClassMethod | NodeKind::ObjectMethod => ctx
                .tree
                .child(member, |k| matches!(k, NodeKind::MethodHeader))
                .unwrap_or(member),
            _ => member,
        };
        ctx.tree
            .child(holder, |k| matches!(k, NodeKind::TypeAnnotation))
            .and_then(|ann| ctx.tree.child(ann, is_type_like))
    })
}

fn is_type_like(kind: &NodeKind) -> bool {
    kind.is_type_node() || matches!(kind, NodeKind::StringLiteralType)
}

/// Collect the string-literal values mentioned in a type annotation.
fn annotation_literal_values(
    type_node: NodeId,
    ctx: &AnalysisContext,
    out: &mut Vec<String>,
    depth: u8,
) {
    if depth > 16 {
        return;
    }
    let tree = ctx.tree;
    match tree.kind(type_node) {
        NodeKind::StringLiteralType => {
            if let Some(value) = tree.escaped_string_literal(type_node) {
                out.push(value);
            }
        }
        NodeKind::UnionType | NodeKind::DefaultUnionType => {
            for child in tree.children_matching(type_node, is_type_like) {
                annotation_literal_values(child, ctx, out, depth + 1);
            }
        }
        NodeKind::NullableType | NodeKind::ParenthesizedType | NodeKind::ConstrainedType => {
            if let Some(inner) = tree.child(type_node, is_type_like) {
                annotation_literal_values(inner, ctx, out, depth + 1);
            }
        }
        NodeKind::DeclaredType => {
            // Alias: expand through to the aliased type body.
            let target = pkl_types::resolver::resolve_declared_type(
                ctx,
                NodeRef::new(ctx.file_id, type_node),
            );
            if let Some(alias) = target
                && let Some(body) = ctx
                    .tree_for(alias.file_id)
                    .and_then(|t| t.type_alias_body(alias.node_id))
            {
                annotation_literal_values(body, ctx, out, depth + 1);
            }
        }
        _ => {}
    }
}

fn literal_item(
    value: &str,
    detailed: &str,
    literal: NodeId,
    ctx: &AnalysisContext,
) -> CompletionItem {
    literal_item_with_alias(value, detailed, None, literal, ctx)
}

fn literal_item_with_alias(
    value: &str,
    detailed: &str,
    alias_rendered: Option<String>,
    literal: NodeId,
    ctx: &AnalysisContext,
) -> CompletionItem {
    let span = ctx.tree.span(literal);
    // Replace only the contents between the quotes (single line).
    let range = Range {
        start: tower_lsp_server::lsp_types::Position {
            line: span.begin_line - 1,
            character: span.begin_col,
        },
        end: tower_lsp_server::lsp_types::Position {
            line: span.end_line - 1,
            character: span.end_col - 1,
        },
    };
    let shown_type = alias_rendered.unwrap_or_else(|| detailed.to_string());
    CompletionItem {
        label: format!("\"{value}\""),
        kind: Some(CompletionItemKind::ENUM_MEMBER),
        label_details: Some(CompletionItemLabelDetails {
            detail: (!shown_type.is_empty()).then_some(shown_type.clone()),
            description: None,
        }),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: value.to_string(),
        })),
        documentation: (!shown_type.is_empty()).then(|| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("```pkl\n{shown_type}\n```"),
            })
        }),
        ..Default::default()
    }
}
