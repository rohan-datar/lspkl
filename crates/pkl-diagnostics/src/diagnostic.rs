//! Single diagnostic emitted by an analyzer

use pkl_analysis::ast::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
            Severity::Hint => "hint",
        }
    }
}

/// A single text-edit operation against the source.
///
/// `span` describes the range to replace. A *zero-length* span — where
/// `begin_byte == end_byte` — represents an insertion at that offset.
/// `new_text` is the literal text to insert/replace with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub span: Span,
    pub new_text: String,
}

impl TextEdit {
    /// Replace the source within `span` with `new_text`.
    pub fn replace(span: Span, new_text: impl Into<String>) -> Self {
        Self {
            span,
            new_text: new_text.into(),
        }
    }

    /// Insert `text` immediately before `position`.
    pub fn insert_before(position: Span, text: impl Into<String>) -> Self {
        let collapsed = Span {
            end_line: position.begin_line,
            end_col: position.begin_col,
            end_byte: position.begin_byte,
            ..position
        };
        Self::replace(collapsed, text)
    }

    /// Insert `text` immediately after `position`.
    pub fn insert_after(position: Span, text: impl Into<String>) -> Self {
        let collapsed = Span {
            begin_line: position.end_line,
            begin_col: position.end_col,
            begin_byte: position.end_byte,
            ..position
        };
        Self::replace(collapsed, text)
    }

    /// `true` when this edit is a pure insertion (zero-length range).
    pub fn is_insertion(&self) -> bool {
        self.span.begin_byte == self.span.end_byte
    }
}

/// A quickfix attached to a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickFix {
    pub title: String,
    pub edits: Vec<TextEdit>,
}

impl QuickFix {
    pub fn new(title: impl Into<String>, edits: Vec<TextEdit>) -> Self {
        Self {
            title: title.into(),
            edits,
        }
    }

    pub fn single(title: impl Into<String>, edit: TextEdit) -> Self {
        Self::new(title, vec![edit])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: Span,
    pub severity: Severity,
    pub message: String,
    pub quick_fixes: Vec<QuickFix>,
}

impl Diagnostic {
    pub fn new(span: Span, severity: Severity, message: impl Into<String>) -> Self {
        Self {
            span,
            severity,
            message: message.into(),
            quick_fixes: Vec::new(),
        }
    }

    pub fn error(span: Span, message: impl Into<String>) -> Self {
        Self::new(span, Severity::Error, message)
    }

    pub fn warning(span: Span, message: impl Into<String>) -> Self {
        Self::new(span, Severity::Warning, message)
    }

    pub fn info(span: Span, message: impl Into<String>) -> Self {
        Self::new(span, Severity::Info, message)
    }

    pub fn hint(span: Span, message: impl Into<String>) -> Self {
        Self::new(span, Severity::Hint, message)
    }

    /// Attach a single quickfix. Chainable.
    pub fn with_fix(mut self, fix: QuickFix) -> Self {
        self.quick_fixes.push(fix);
        self
    }

    /// Replace all attached quickfixes.
    pub fn with_fixes(mut self, fixes: Vec<QuickFix>) -> Self {
        self.quick_fixes = fixes;
        self
    }
}
