mod harness;

use lspkl_syntax::ast::NodeKind;
use lspkl_syntax::parse;

#[test]
fn name_finds_identifier_inside_method_header() {
    let src = "function foo(x: Int): Int = x + 1";
    let tree = parse(src).unwrap();
    let method = tree.children_matching(tree.root(), |k| matches!(k, NodeKind::ClassMethod))[0];
    assert_eq!(tree.name(method), Some("foo"));
}

#[test]
fn doc_comments_returns_stripped_source_slices() {
    let src = "/// Docs for x.\n/// Second line.\nx: Int = 1\ny: Int = 2\n";
    let tree = parse(src).unwrap();
    let prop = tree.children_matching(tree.root(), |k| matches!(k, NodeKind::ClassProperty))[0];
    let docs = tree.doc_comments(prop);
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0], "/// Docs for x.\n/// Second line.");

    // `y` has no doc comment of its own.
    let y = tree.children_matching(tree.root(), |k| matches!(k, NodeKind::ClassProperty))[1];
    assert!(tree.doc_comments(y).is_empty());
}

#[test]
fn byte_ranges_and_line_index_preserve_unicode_and_crlf() {
    let source = "// é😀\r\nvalue = 1\r\n";
    let tree = parse(source).unwrap();
    let property = tree.children_matching(tree.root(), |k| matches!(k, NodeKind::ClassProperty))[0];
    let name = tree.identifier(property).unwrap();
    let range = tree.span(name);
    assert_eq!(&source[std::ops::Range::<usize>::from(range)], "value");
    assert_eq!(tree.text(name), "value");
    assert_eq!(
        tree.line_index().line_col(range.start()),
        lspkl_syntax::LineCol { line: 1, col: 0 }
    );
    assert_eq!(tree.find_at(range.start()), Some(name));
    assert_eq!(tree.find_at(range.end()), Some(name));
}

#[test]
fn cursor_lookup_includes_eof_and_rejects_offsets_past_it() {
    for source in ["", "value = 1", "value = 1\n", "value = (", "value ="] {
        let tree = parse(source).unwrap();
        let end = lspkl_syntax::TextSize::try_from(source.len()).unwrap();
        assert!(
            tree.find_at(end).is_some(),
            "EOF must remain addressable: {source:?}"
        );
        assert!(
            tree.find_at(end + lspkl_syntax::TextSize::from(1))
                .is_none()
        );
    }
}
