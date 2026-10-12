//! Render a diagnostic list to a stable, human-readable string for snapshot
//! testing.

use pkl_syntax::LineIndex;
use std::fmt::Write;

use crate::diagnostic::Diagnostic;

pub fn render_diagnostics(diagnostics: &[Diagnostic], line_index: &LineIndex) -> String {
    if diagnostics.is_empty() {
        return String::from("(no diagnostics)\n");
    }

    let mut sorted: Vec<&Diagnostic> = diagnostics.iter().collect();
    sorted.sort_by_key(|d| (d.span.start(), d.severity.as_str()));

    let mut out = String::new();
    for d in sorted {
        let start = line_index.line_col(d.span.start());
        writeln!(
            &mut out,
            "{}:{} {}: {}",
            start.line + 1,
            start.col + 1,
            d.severity.as_str(),
            d.message
        )
        .unwrap();

        // Attached quickfixes render under the diagnostic, indented. We
        // emit each edit as `insert "<text>" at L:C` (or `replace L:C-L:C
        // with "<text>"`) so snapshots can spot regressions. Existing
        // snapshots stay byte-stable because diagnostics without fixes
        // print no extra lines.
        for fix in &d.quick_fixes {
            writeln!(&mut out, "  fix: {}", fix.title).unwrap();
            for edit in &fix.edits {
                let start = line_index.line_col(edit.span.start());
                let end = line_index.line_col(edit.span.end());
                if edit.is_insertion() {
                    writeln!(
                        &mut out,
                        "    insert {:?} at {}:{}",
                        edit.new_text,
                        start.line + 1,
                        start.col + 1,
                    )
                    .unwrap();
                } else {
                    writeln!(
                        &mut out,
                        "    replace {}:{}-{}:{} with {:?}",
                        start.line + 1,
                        start.col + 1,
                        end.line + 1,
                        end.col + 1,
                        edit.new_text,
                    )
                    .unwrap();
                }
            }
        }
    }
    out
}
