//! Completion: suggest identifiers / members / keywords at the cursor.
//!
//! Resource-aware module URI candidates and their lazy documentation resolution
//! are provided by `module_uri_completion`, before these semantic providers.
//!
//! TODO(completion-doc-link): `DocCommentMemberLinkCompletionProvider` is
//! deferred

use lsp_types::{CompletionItem, CompletionItemKind, CompletionResponse, InsertTextFormat, Range};

use lspkl_db::{Db, File};
use lspkl_syntax::{TextRange, TextSize};
use lspkl_types::AnalysisContext;

mod implement_method;
mod qualified;
mod string_literal;
mod unqualified;

/// Compute completion candidates at a file-qualified UTF-8 byte offset.
pub fn completion(
    db: &dyn Db,
    root: File,
    offset: lspkl_types::InFile<TextSize>,
    range_for: impl Fn(TextRange) -> Range,
) -> Option<CompletionResponse> {
    let ctx = &AnalysisContext::for_root(db, root);
    // Deepest node containing the cursor; whitespace past any token yields
    // `None`, which propagates as "no completions".
    let file = offset.file;
    let tree = ctx.tree(file)?;
    let offset = offset.value;
    let node = tree.find_at(offset)?;

    // URI completion runs before this semantic dispatch.
    // Each provider contributes only when applicable to the current node.
    let mut collector = Vec::new();
    collector.extend(qualified::completions(node, tree, file, ctx));
    collector.extend(unqualified::completions(node, tree, file, ctx, offset));
    collector.extend(string_literal::completions(
        node, tree, file, ctx, &range_for,
    ));
    collector.extend(implement_method::completions(node, tree, file, ctx));
    // TODO(completion-doc-link): DocCommentMemberLinkCompletionProvider slot.

    if collector.is_empty() {
        return None;
    }

    collector.dedup_by(|a, b| a.label == b.label);
    Some(CompletionResponse::Array(collector))
}

/// Walk up from `node` (inclusive) until `pred` matches.
pub(crate) fn ancestor_or_self(
    node: lspkl_syntax::ast::NodeId,
    tree: &lspkl_syntax::ast::SyntaxTree,
    pred: impl Fn(&lspkl_syntax::ast::NodeKind) -> bool,
) -> Option<lspkl_syntax::ast::NodeId> {
    let mut current = Some(node);
    while let Some(id) = current {
        if pred(tree.kind(id)) {
            return Some(id);
        }
        current = tree.parent(id);
    }
    None
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
