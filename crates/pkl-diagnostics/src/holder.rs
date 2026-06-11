//! Collects diagnostics emitted by analyzers during a single tree walk.

use pkl_analysis::ast::Span;

use crate::diagnostic::{Diagnostic, QuickFix, Severity};

#[derive(Debug, Default)]
pub struct DiagnosticsHolder {
    diagnostics: Vec<Diagnostic>,
}

impl DiagnosticsHolder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, diag: Diagnostic) {
        self.diagnostics.push(diag);
    }

    pub fn add_error(&mut self, span: Span, message: impl Into<String>) {
        self.add(Diagnostic::new(span, Severity::Error, message));
    }

    pub fn add_warning(&mut self, span: Span, message: impl Into<String>) {
        self.add(Diagnostic::new(span, Severity::Warning, message));
    }

    pub fn add_info(&mut self, span: Span, message: impl Into<String>) {
        self.add(Diagnostic::new(span, Severity::Info, message));
    }

    pub fn add_hint(&mut self, span: Span, message: impl Into<String>) {
        self.add(Diagnostic::new(span, Severity::Hint, message));
    }

    pub fn add_error_with_fix(&mut self, span: Span, message: impl Into<String>, fix: QuickFix) {
        self.add(Diagnostic::error(span, message).with_fix(fix));
    }

    pub fn add_warning_with_fix(&mut self, span: Span, message: impl Into<String>, fix: QuickFix) {
        self.add(Diagnostic::warning(span, message).with_fix(fix));
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }
}
