//! Render a diagnostic list to a stable, human-readable string for snapshot
//! testing.

use std::fmt::Write;

use crate::diagnostic::Diagnostic;

pub fn render_diagnostics(diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return String::from("(no diagnostics)\n");
    }

    let mut sorted: Vec<&Diagnostic> = diagnostics.iter().collect();
    sorted.sort_by_key(|d| (d.span.begin_line, d.span.begin_col, d.severity.as_str()));

    let mut out = String::new();
    for d in sorted {
        writeln!(
            &mut out,
            "{}:{} {}: {}",
            d.span.begin_line,
            d.span.begin_col,
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
                if edit.is_insertion() {
                    writeln!(
                        &mut out,
                        "    insert {:?} at {}:{}",
                        edit.new_text, edit.span.begin_line, edit.span.begin_col,
                    )
                    .unwrap();
                } else {
                    writeln!(
                        &mut out,
                        "    replace {}:{}-{}:{} with {:?}",
                        edit.span.begin_line,
                        edit.span.begin_col,
                        edit.span.end_line,
                        edit.span.end_col,
                        edit.new_text,
                    )
                    .unwrap();
                }
            }
        }
    }
    out
}
