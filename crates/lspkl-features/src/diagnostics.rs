//! Diagnostics: the LSP wire form published to clients.
//!
//! Clients echo published diagnostics back in code-action requests;
//! [`crate::code_action`] matches them by message and range, so publication and
//! matching must agree on this form.

use lsp_types::{DiagnosticSeverity, Range};
use lspkl_diagnostics::{Diagnostic, Severity};

/// Convert a diagnostic whose span the caller has already converted to `range`.
pub fn to_lsp(diagnostic: &Diagnostic, range: Range) -> lsp_types::Diagnostic {
    lsp_types::Diagnostic {
        range,
        severity: Some(match diagnostic.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
            Severity::Info => DiagnosticSeverity::INFORMATION,
            Severity::Hint => DiagnosticSeverity::HINT,
        }),
        source: Some("lspkl".to_owned()),
        message: diagnostic.message.clone(),
        ..lsp_types::Diagnostic::default()
    }
}
