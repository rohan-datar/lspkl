//! The protocol boundary between encoded LSP positions and UTF-8 byte offsets.

use lsp_types::{Position, Range};
use lspkl_syntax::{LineCol, TextRange, TextSize, WideEncoding, WideLineCol, ast::SyntaxTree};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PositionEncoding {
    Utf8,
    #[default]
    Utf16,
}

pub struct PositionConverter<'a> {
    tree: &'a SyntaxTree,
    encoding: PositionEncoding,
}

impl<'a> PositionConverter<'a> {
    pub fn new(tree: &'a SyntaxTree, encoding: PositionEncoding) -> Self {
        Self { tree, encoding }
    }

    fn line_end(&self, line: u32) -> Option<TextSize> {
        let range = self.tree.line_index().line(line)?;
        let bytes = self.tree.source().as_bytes();
        let mut end = usize::from(range.end());
        if end > usize::from(range.start()) && bytes[end - 1] == b'\n' {
            end -= 1;
            if end > usize::from(range.start()) && bytes[end - 1] == b'\r' {
                end -= 1;
            }
        }
        TextSize::try_from(end).ok()
    }

    /// Invalid lines and positions inside a character are rejected; columns
    /// beyond the line's content are clamped as required by LSP.
    pub fn offset(&self, position: Position) -> Option<TextSize> {
        let index = self.tree.line_index();
        let end = self.line_end(position.line)?;
        let line_width = self.position(end).character;
        let col = position.character.min(line_width);
        let line_col = match self.encoding {
            PositionEncoding::Utf8 => LineCol {
                line: position.line,
                col,
            },
            PositionEncoding::Utf16 => index.to_utf8(
                WideEncoding::Utf16,
                WideLineCol {
                    line: position.line,
                    col,
                },
            )?,
        };
        let offset = index.offset(line_col)?;
        (offset <= end && self.tree.source().is_char_boundary(usize::from(offset)))
            .then_some(offset)
    }

    /// Convert a valid semantic byte offset to the negotiated wire encoding.
    pub fn position(&self, offset: TextSize) -> Position {
        let index = self.tree.line_index();
        let line_col = index.line_col(offset);
        let end = self
            .line_end(line_col.line)
            .expect("offset has a valid line");
        let line_col = index.line_col(offset.min(end));
        let character = match self.encoding {
            PositionEncoding::Utf8 => line_col.col,
            PositionEncoding::Utf16 => {
                index
                    .to_wide(WideEncoding::Utf16, line_col)
                    .expect("valid byte column converts to UTF-16")
                    .col
            }
        };
        Position::new(line_col.line, character)
    }

    pub fn range(&self, range: TextRange) -> Range {
        Range::new(self.position(range.start()), self.position(range.end()))
    }

    pub fn text_range(&self, range: Range) -> Option<TextRange> {
        let start = self.offset(range.start)?;
        let end = self.offset(range.end)?;
        (start <= end).then(|| TextRange::new(start, end))
    }
}
