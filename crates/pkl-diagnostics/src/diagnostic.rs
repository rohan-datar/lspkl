//! Single diagnostic emitted by an analyzer

use pkl_syntax::TextRange;

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
/// `span` describes the byte range to replace. A zero-length range
/// represents an insertion at that offset.
/// `new_text` is the literal text to insert/replace with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub span: TextRange,
    pub new_text: String,
}

impl TextEdit {
    /// Replace the source within `span` with `new_text`.
    pub fn replace(span: TextRange, new_text: impl Into<String>) -> Self {
        Self {
            span,
            new_text: new_text.into(),
        }
    }

    /// Insert `text` immediately before `position`.
    pub fn insert_before(position: TextRange, text: impl Into<String>) -> Self {
        let collapsed = TextRange::empty(position.start());
        Self::replace(collapsed, text)
    }

    /// Insert `text` immediately after `position`.
    pub fn insert_after(position: TextRange, text: impl Into<String>) -> Self {
        let collapsed = TextRange::empty(position.end());
        Self::replace(collapsed, text)
    }

    /// `true` when this edit is a pure insertion (zero-length range).
    pub fn is_insertion(&self) -> bool {
        self.span.is_empty()
    }
}

/// An editor command attached to a quickfix, independent of project I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickFixCommand {
    DownloadPackage { uri: String },
}
/// A quickfix attached to a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickFix {
    pub title: String,
    pub edits: Vec<TextEdit>,
    pub command: Option<QuickFixCommand>,
}

impl QuickFix {
    pub fn new(title: impl Into<String>, edits: Vec<TextEdit>) -> Self {
        Self {
            title: title.into(),
            edits,
            command: None,
        }
    }

    pub fn single(title: impl Into<String>, edit: TextEdit) -> Self {
        Self::new(title, vec![edit])
    }
    pub fn download_package(uri: String) -> Self {
        Self {
            title: format!("Download package `{uri}`"),
            edits: Vec::new(),
            command: Some(QuickFixCommand::DownloadPackage { uri }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: TextRange,
    pub severity: Severity,
    pub message: String,
    pub quick_fixes: Vec<QuickFix>,
}

impl Diagnostic {
    pub fn new(span: TextRange, severity: Severity, message: impl Into<String>) -> Self {
        Self {
            span,
            severity,
            message: message.into(),
            quick_fixes: Vec::new(),
        }
    }

    pub fn error(span: TextRange, message: impl Into<String>) -> Self {
        Self::new(span, Severity::Error, message)
    }

    pub fn warning(span: TextRange, message: impl Into<String>) -> Self {
        Self::new(span, Severity::Warning, message)
    }

    pub fn info(span: TextRange, message: impl Into<String>) -> Self {
        Self::new(span, Severity::Info, message)
    }

    pub fn hint(span: TextRange, message: impl Into<String>) -> Self {
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
