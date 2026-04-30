mod harness;

use std::fs;

use pkl_analysis::parse;

#[test]
fn parse_all_diagnostic_inputs_without_crashing() {
    if harness::skip_if_missing_diagnostics_inputs() {
        eprintln!("diagnostic input fixtures are not present yet; skipping skeleton harness");
        return;
    }

    let input_dir = harness::diagnostics_input_dir();
    let files = harness::collect_pkl_files(&input_dir);
    assert!(
        !files.is_empty(),
        "expected at least one diagnostic input under {}",
        input_dir.display()
    );

    for input_path in files {
        let relative = input_path
            .strip_prefix(&input_dir)
            .expect("input path must be under diagnostics input directory");
        let source = fs::read_to_string(&input_path).expect("input fixture should be readable");
        if let Err(err) = parse(&source) {
            panic!(
                "parser failed for diagnostics fixture {}: {err}",
                relative.display()
            );
        }
    }
}
