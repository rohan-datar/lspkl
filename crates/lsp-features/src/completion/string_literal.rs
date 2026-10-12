//! String-literal type completion: suggest allowed literal values.

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionTextEdit,
    Documentation, MarkupContent, MarkupKind, Range, TextEdit,
};

use pkl_syntax::ast::nodes::{
    ClassMethod, ClassProperty, MethodHeader, ObjectMethod, ObjectProperty, TypeAlias,
    TypeAnnotation, TypeNode,
};
use pkl_syntax::ast::{NodeId, NodeKind};
use pkl_syntax::{TextRange, TextSize};
use pkl_types::infer_expr_type::infer_expr_type_from_context;
use pkl_types::{AnalysisContext, NodeRef, Type, TypeBindings};

pub(crate) fn completions(
    node: NodeId,
    ctx: &AnalysisContext,
    range_for: &impl Fn(TextRange) -> Range,
) -> Vec<CompletionItem> {
    let Some(literal) = find_string_literal(node, ctx) else {
        return Vec::new();
    };

    // Preferred path: the shared context inference (handles object-entry
    // value positions, argument positions, etc.).
    let expected = infer_expr_type_from_context(literal, &TypeBindings::new(), ctx);
    let mut out = Vec::new();
    let detailed = render_detail(&expected, ctx);
    expand(
        &expected, None, &detailed, literal, ctx, range_for, &mut out,
    );
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
    annotation_literal_values(NodeRef::new(ctx.file_id, annotation), ctx, &mut values, 0);
    for value in values {
        out.push(literal_item(&value, &detail_text, literal, ctx, range_for));
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
    range_for: &impl Fn(TextRange) -> Range,
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
                range_for,
            ));
        }
        Type::Union { left, right, .. } => {
            expand(left, original_alias, detailed, literal, ctx, range_for, out);
            expand(
                right,
                original_alias,
                detailed,
                literal,
                ctx,
                range_for,
                out,
            );
        }
        Type::Alias { .. } => expand(
            &result_type.unaliased(ctx),
            Some(result_type),
            detailed,
            literal,
            ctx,
            range_for,
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
        let tree = ctx.tree;
        let annotation = match tree.kind(member) {
            NodeKind::ClassProperty => ClassProperty::cast(tree, member)?.type_annotation(tree),
            NodeKind::ObjectProperty => ObjectProperty::cast(tree, member)?.type_annotation(tree),
            NodeKind::ClassMethod => ClassMethod::cast(tree, member)?
                .header(tree)
                .and_then(|header| MethodHeader::cast(tree, header)?.type_annotation(tree)),
            NodeKind::ObjectMethod => ObjectMethod::cast(tree, member)?
                .header(tree)
                .and_then(|header| MethodHeader::cast(tree, header)?.type_annotation(tree)),
            _ => None,
        }?;
        TypeAnnotation::cast(tree, annotation)?.ty(tree)
    })
}

/// Collect the string-literal values mentioned in a type annotation.
fn annotation_literal_values(
    type_node: NodeRef,
    ctx: &AnalysisContext,
    out: &mut Vec<String>,
    depth: u8,
) {
    if depth > 16 {
        return;
    }
    let Some(tree) = ctx.tree_for(type_node.file_id) else {
        return;
    };
    match TypeNode::cast(tree, type_node.node_id) {
        Some(TypeNode::StringLiteralType(literal)) => {
            if let Some(value) = tree.escaped_string_literal(literal.id()) {
                out.push(value);
            }
        }
        Some(TypeNode::UnionType(union)) => {
            for child in [union.left(tree), union.right(tree)].into_iter().flatten() {
                annotation_literal_values(
                    NodeRef::new(type_node.file_id, child),
                    ctx,
                    out,
                    depth + 1,
                );
            }
        }
        Some(
            ty @ (TypeNode::DefaultUnionType(_)
            | TypeNode::NullableType(_)
            | TypeNode::ParenthesizedType(_)
            | TypeNode::ConstrainedType(_)),
        ) => {
            let inner = match ty {
                TypeNode::DefaultUnionType(ty) => ty.inner(tree),
                TypeNode::NullableType(ty) => ty.inner(tree),
                TypeNode::ParenthesizedType(ty) => ty.inner(tree),
                TypeNode::ConstrainedType(ty) => ty.inner(tree),
                _ => unreachable!(),
            };
            if let Some(inner) = inner {
                annotation_literal_values(
                    NodeRef::new(type_node.file_id, inner),
                    ctx,
                    out,
                    depth + 1,
                );
            }
        }
        Some(TypeNode::DeclaredType(_)) => {
            // Alias: preserve its file identity while expanding the type body.
            if let Some(alias) = pkl_types::resolver::resolve_declared_type(ctx, type_node)
                && let Some(body) = ctx
                    .tree_for(alias.file_id)
                    .and_then(|tree| TypeAlias::cast(tree, alias.node_id)?.ty(tree))
            {
                annotation_literal_values(NodeRef::new(alias.file_id, body), ctx, out, depth + 1);
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
    range_for: &impl Fn(TextRange) -> Range,
) -> CompletionItem {
    literal_item_with_alias(value, detailed, None, literal, ctx, range_for)
}

fn literal_item_with_alias(
    value: &str,
    detailed: &str,
    alias_rendered: Option<String>,
    literal: NodeId,
    ctx: &AnalysisContext,
    range_for: &impl Fn(TextRange) -> Range,
) -> CompletionItem {
    let span = ctx.tree.span(literal);
    let text = ctx.tree.text(literal);
    // Preserve both delimiters, including raw-string hash delimiters.
    let opening = text.find('"').map_or(0, |quote| quote + 1);
    let closing = opening;
    let has_closing = opening > 0
        && text.len() >= opening + closing
        && text.as_bytes()[text.len() - closing] == b'"'
        && text[text.len() - closing + 1..] == text[..opening.saturating_sub(1)];
    let start = span.start() + TextSize::try_from(opening).expect("literal fits in TextSize");
    let end = if has_closing {
        span.end() - TextSize::try_from(closing).expect("delimiter fits in TextSize")
    } else {
        span.end()
    };
    let range = range_for(TextRange::new(start, end));
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
