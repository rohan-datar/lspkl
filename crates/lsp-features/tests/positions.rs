use lsp_features::positions::{PositionConverter, PositionEncoding};
use lsp_types::{Position, Range};
use pkl_syntax::{TextRange, TextSize};

#[test]
fn unicode_boundaries_reject_interior_code_units() {
    let tree = pkl_syntax::parse("a😀éz\n").unwrap();
    let utf8 = PositionConverter::new(&tree, PositionEncoding::Utf8);
    let utf16 = PositionConverter::new(&tree, PositionEncoding::Utf16);
    for col in [2, 3, 4, 6] {
        assert_eq!(utf8.offset(Position::new(0, col)), None);
    }
    assert_eq!(utf16.offset(Position::new(0, 2)), None);
    assert_eq!(utf16.offset(Position::new(0, 3)), Some(TextSize::from(5)));
    assert_eq!(utf8.position(TextSize::from(7)), Position::new(0, 7));
    assert_eq!(utf16.position(TextSize::from(7)), Position::new(0, 4));
    let bytes = TextRange::new(TextSize::from(1), TextSize::from(7));
    let wire = Range::new(Position::new(0, 1), Position::new(0, 4));
    assert_eq!(utf16.range(bytes), wire);
    assert_eq!(utf16.text_range(wire), Some(bytes));
    assert_eq!(
        utf16.text_range(Range::new(Position::new(0, 2), Position::new(0, 4))),
        None
    );
    assert_eq!(
        utf16.text_range(Range::new(Position::new(0, 4), Position::new(0, 1))),
        None
    );
}

#[test]
fn columns_clamp_before_crlf_and_final_empty_line_is_valid() {
    let tree = pkl_syntax::parse("é😀\r\nx\r\n").unwrap();
    for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
        let converter = PositionConverter::new(&tree, encoding);
        assert_eq!(
            converter.offset(Position::new(0, u32::MAX)),
            Some(TextSize::from(6))
        );
        assert_eq!(
            converter.offset(Position::new(1, u32::MAX)),
            Some(TextSize::from(9))
        );
        assert_eq!(
            converter.offset(Position::new(2, u32::MAX)),
            Some(TextSize::from(11))
        );
        assert_eq!(converter.offset(Position::new(3, 0)), None);
        assert_eq!(converter.position(TextSize::from(11)), Position::new(2, 0));
    }
    let tree = pkl_syntax::parse("").unwrap();
    let converter = PositionConverter::new(&tree, Default::default());
    assert_eq!(
        converter.offset(Position::new(0, 100)),
        Some(TextSize::from(0))
    );
    assert_eq!(converter.offset(Position::new(1, 0)), None);
}
