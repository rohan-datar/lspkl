mod harness;

use std::fs;

use pkl_syntax::parse;

#[test]
fn parser_snapshot_tests() {
    if harness::skip_if_missing_parser_fixtures() {
        eprintln!("parser snapshot fixtures are not present yet; skipping skeleton harness");
        return;
    }

    let input_dir = harness::parser_snapshot_input_dir();
    let files = harness::collect_pkl_files(&input_dir);
    assert!(
        !files.is_empty(),
        "expected at least one parser snapshot input under {}",
        input_dir.display()
    );

    for input_path in files {
        let relative = input_path
            .strip_prefix(&input_dir)
            .expect("input path must be under parser input directory");
        let source = fs::read_to_string(&input_path).expect("input fixture should be readable");
        let tree = parse(&source).unwrap_or_else(|err| {
            panic!("failed to parse {}: {err}", relative.display());
        });
        let rendered = harness::render_ast(&tree);
        let snapshot_name = relative
            .display()
            .to_string()
            .replace(['/', '\\', '.'], "_");
        insta::assert_snapshot!(snapshot_name, rendered);
    }
}
