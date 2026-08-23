//! Completion: suggest identifiers / members / keywords at the cursor.
//!
//! TODO(completion-module-uri): `ModuleUriCompletionProvider` is deferred —
//! completing module URIs (`import "…"` / `amends "…"`) needs the Phase 6
//! package/project machinery to enumerate resolvable modules.
//!
//! TODO(completion-doc-link): `DocCommentMemberLinkCompletionProvider` is
//! deferred

use tower_lsp_server::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionParams, CompletionResponse, InsertTextFormat,
};

use pkl_types::AnalysisContext;

mod implement_method;
mod members;
mod qualified;
mod string_literal;
mod unqualified;

/// Compute completion candidates for the cursor location in `params`.
pub fn completion(ctx: &AnalysisContext, params: &CompletionParams) -> Option<CompletionResponse> {
    let line = params.text_document_position.position.line + 1;
    let col = params.text_document_position.position.character + 1;

    // Deepest node containing the cursor; whitespace past any token yields
    // `None`, which propagates as "no completions".
    let node = ctx.tree.find_at(line, col)?;

    // Provider order mirrors CompletionFeature.kt:28-36 (minus the two
    // deferred providers noted in the module docs). Each provider decides
    // applicability from the node kind and contributes zero items otherwise.
    //
    // TODO(completion-module-uri): ModuleUriCompletionProvider slot.
    let mut collector = Vec::new();
    collector.extend(qualified::completions(node, ctx));
    collector.extend(unqualified::completions(node, ctx, col));
    collector.extend(string_literal::completions(node, ctx));
    collector.extend(implement_method::completions(node, ctx));
    // TODO(completion-doc-link): DocCommentMemberLinkCompletionProvider slot.

    if collector.is_empty() {
        return None;
    }

    collector.dedup_by(|a, b| a.label == b.label);
    Some(CompletionResponse::Array(collector))
}

/// Walk up from `node` (inclusive) until `pred` matches.
pub(crate) fn ancestor_or_self(
    node: pkl_syntax::ast::NodeId,
    ctx: &AnalysisContext,
    pred: impl Fn(&pkl_syntax::ast::NodeKind) -> bool,
) -> Option<pkl_syntax::ast::NodeId> {
    let mut current = Some(node);
    while let Some(id) = current {
        if pred(ctx.tree.kind(id)) {
            return Some(id);
        }
        current = ctx.tree.parent(id);
    }
    None
}

/// Lazy-resolve detail/documentation for a `CompletionItem` that was
/// returned by `completion` without those fields populated.
pub fn resolve_completion_item(item: CompletionItem) -> CompletionItem {
    // TODO(completion-module-uri): route URI items through package machinery
    item
}

// ---------------------------------------------------------------------------
// Shared item-building helpers used by the providers.
// ---------------------------------------------------------------------------

/// Build a plain completion item with optional detail/kind.
pub(crate) fn simple_item(
    label: &str,
    insert: &str,
    detail: &str,
    kind: Option<CompletionItemKind>,
) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        detail: (!detail.is_empty()).then(|| detail.to_string()),
        kind,
        insert_text: Some(insert.to_string()),
        insert_text_format: insert.contains('$').then_some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }
}

/// Expression-level keywords
pub(crate) fn expression_level_keywords() -> Vec<CompletionItem> {
    [
        ("as", "$1 as $2", "Type cast", CompletionItemKind::SNIPPET),
        ("else", "else ", "Else clause", CompletionItemKind::SNIPPET),
        ("false", "false", "Boolean", CompletionItemKind::CONSTANT),
        (
            "if",
            "if ($1) $2 else $3",
            "If-then-else",
            CompletionItemKind::SNIPPET,
        ),
        (
            "import",
            "import(\"$1\")",
            "Import expression",
            CompletionItemKind::SNIPPET,
        ),
        (
            "import*",
            "import*(\"$1\")",
            "Import glob",
            CompletionItemKind::SNIPPET,
        ),
        ("is", "$1 is $2", "Type check", CompletionItemKind::SNIPPET),
        (
            "let",
            "let ($1 = $2) $3",
            "Let variable",
            CompletionItemKind::SNIPPET,
        ),
        ("module", "module", "Module", CompletionItemKind::CONSTANT),
        (
            "new",
            "new $1 {}",
            "New object",
            CompletionItemKind::SNIPPET,
        ),
        ("null", "null", "Null", CompletionItemKind::CONSTANT),
        (
            "outer",
            "outer",
            "Access outer scope",
            CompletionItemKind::CONSTANT,
        ),
        (
            "read",
            "read($1)",
            "Read expression",
            CompletionItemKind::SNIPPET,
        ),
        (
            "read?",
            "read?($1)",
            "Read or null",
            CompletionItemKind::SNIPPET,
        ),
        (
            "read*",
            "read*($1)",
            "Read glob",
            CompletionItemKind::SNIPPET,
        ),
        (
            "super",
            "super",
            "Super class",
            CompletionItemKind::CONSTANT,
        ),
        ("this", "this", "This", CompletionItemKind::CONSTANT),
        ("throw", "throw($1)", "Throw", CompletionItemKind::SNIPPET),
        ("trace", "trace($1)", "Trace", CompletionItemKind::SNIPPET),
        ("true", "true", "Boolean", CompletionItemKind::CONSTANT),
    ]
    .into_iter()
    .map(|(label, insert, detail, kind)| simple_item(label, insert, detail, Some(kind)))
    .collect()
}

/// Definition-level keywords
pub(crate) fn definition_level_keywords() -> Vec<CompletionItem> {
    [
        (
            "for",
            "for ($1 in $2) {\n  $3\n}",
            "For generator",
            CompletionItemKind::SNIPPET,
        ),
        (
            "function",
            "function $1($2) = $3",
            "Function",
            CompletionItemKind::SNIPPET,
        ),
        ("hidden", "hidden ", "", CompletionItemKind::KEYWORD),
        ("local", "local ", "", CompletionItemKind::KEYWORD),
        ("fixed", "fixed ", "", CompletionItemKind::KEYWORD),
        ("const", "const ", "", CompletionItemKind::KEYWORD),
        (
            "when",
            "when ($1) {\n  $2\n}",
            "When generator",
            CompletionItemKind::SNIPPET,
        ),
    ]
    .into_iter()
    .map(|(label, insert, detail, kind)| simple_item(label, insert, detail, Some(kind)))
    .collect()
}

/// Module-level keywords
pub(crate) fn module_level_keywords() -> Vec<CompletionItem> {
    [
        ("abstract", "abstract ", "", CompletionItemKind::KEYWORD),
        ("amends", "amends ", "", CompletionItemKind::KEYWORD),
        ("class", "class ", "", CompletionItemKind::KEYWORD),
        ("extends", "extends ", "", CompletionItemKind::KEYWORD),
        (
            "import",
            "import \"$1\"",
            "Import clause",
            CompletionItemKind::SNIPPET,
        ),
        (
            "import*",
            "import* \"$1\"",
            "Import glob",
            CompletionItemKind::SNIPPET,
        ),
        ("module", "module ", "", CompletionItemKind::KEYWORD),
        ("open", "open ", "", CompletionItemKind::KEYWORD),
        ("typealias", "typealias ", "", CompletionItemKind::KEYWORD),
    ]
    .into_iter()
    .map(|(label, insert, detail, kind)| simple_item(label, insert, detail, Some(kind)))
    .collect()
}

/// Class-level keywords
pub(crate) fn class_level_keywords() -> Vec<CompletionItem> {
    vec![simple_item(
        "abstract",
        "abstract ",
        "",
        Some(CompletionItemKind::KEYWORD),
    )]
}
