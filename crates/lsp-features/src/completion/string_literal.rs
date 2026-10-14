//! String-literal type completion: suggest allowed literal values.

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionTextEdit,
    Documentation, MarkupContent, MarkupKind, Range, TextEdit,
};

use pkl_syntax::ast::{NodeId, NodeKind};
use pkl_syntax::{TextRange, TextSize};

use pkl_types::{AnalysisContext, NodeRef, Type};

pub(crate) fn completions(
    node: NodeId,
    tree: &pkl_syntax::ast::SyntaxTree,
    file: pkl_db::File,
    ctx: &AnalysisContext,
    range_for: &impl Fn(TextRange) -> Range,
) -> Vec<CompletionItem> {
    let Some(literal) = find_string_literal(node, tree) else {
        return Vec::new();
    };

    // Preferred path: the shared context inference (handles object-entry
    // value positions, argument positions, etc.).
    let expected = NodeRef::new(file, literal)
        .cast(ctx, pkl_syntax::ast::nodes::Expr::cast)
        .map_or(Type::Unknown, |expr| {
            pkl_types::expected_type(ctx.db, expr.map(|expr| expr.id()), ctx.root)
        });
    let mut out = Vec::new();
    let detailed = render_detail(&expected, ctx);
    expand(
        &expected,
        None,
        &detailed,
        NodeRef::new(file, literal),
        ctx,
        range_for,
        &mut out,
    );
    if !out.is_empty() {
        return out;
    }

    // Recover the nearest declaration through the shared scope rather than
    // recursively expanding annotation syntax and alias bodies.
    let scope = pkl_types::scope_at(NodeRef::new(file, literal), ctx);
    if let Some(member) = super::ancestor_or_self(literal, tree, |kind| {
        matches!(
            kind,
            NodeKind::ClassProperty
                | NodeKind::ObjectProperty
                | NodeKind::ClassMethod
                | NodeKind::ObjectMethod
        )
    }) && let Some(name) = tree.name(member)
        && let Some(entry) = scope.lookup(name)
    {
        let ty = entry.ty(ctx);
        let detail = render_detail(&ty, ctx);
        expand(
            &ty,
            None,
            &detail,
            NodeRef::new(file, literal),
            ctx,
            range_for,
            &mut out,
        );
    }
    out
}

fn find_string_literal(node: NodeId, tree: &pkl_syntax::ast::SyntaxTree) -> Option<NodeId> {
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
    literal: NodeRef,
    ctx: &AnalysisContext,
    range_for: &impl Fn(TextRange) -> Range,
    out: &mut Vec<CompletionItem>,
) {
    let Some(tree) = ctx.tree(literal.file) else {
        return;
    };
    match result_type.unconstrained() {
        Type::StringLiteral { value, .. } => {
            out.push(literal_item_with_alias(
                value,
                detailed,
                original_alias.map(|t| t.render(ctx.db)),
                literal.value,
                tree,
                range_for,
            ));
        }
        Type::Union { members } => {
            for member in members.iter() {
                expand(
                    member,
                    original_alias,
                    detailed,
                    literal,
                    ctx,
                    range_for,
                    out,
                );
            }
        }
        Type::Alias { .. } => expand(
            &result_type.unaliased(ctx.db),
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
        other => other.render(ctx.db),
    }
}

fn literal_item_with_alias(
    value: &str,
    detailed: &str,
    alias_rendered: Option<String>,
    literal: NodeId,
    tree: &pkl_syntax::ast::SyntaxTree,
    range_for: &impl Fn(TextRange) -> Range,
) -> CompletionItem {
    let span = tree.span(literal);
    let text = tree.text(literal);
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
