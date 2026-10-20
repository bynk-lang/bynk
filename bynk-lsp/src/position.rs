//! Byte-offset ↔ LSP position conversion.
//!
//! Bynk source spans are byte offsets into the UTF-8 source. LSP positions
//! use UTF-16 code units (per the protocol's default position encoding).
//! For ASCII-only Bynk sources the two agree, but we go through code points
//! to handle multi-byte characters correctly in identifiers and strings.

use bynk_syntax::span::{LineIndex, Span};
use tower_lsp::lsp_types::{Position, Range};

/// A source snapshot paired with its precomputed [`LineIndex`], so a request
/// that maps many spans (semantic tokens, folding ranges, diagnostics, inlay
/// hints, document symbols) pays the O(n) line-scan **once** and then converts
/// each offset in O(log n) — closing the O(n²) blow-up of #732.
///
/// Positions match [`offset_to_position`] exactly (0-based line, UTF-16-code-unit
/// columns), so callers can swap a free-function call for a `PositionMap` method
/// without changing observable output.
pub struct PositionMap<'a> {
    source: &'a str,
    index: LineIndex,
}

impl<'a> PositionMap<'a> {
    /// Build the index for `source` in one pass. Reuse the returned map for
    /// every conversion over this snapshot.
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            index: LineIndex::new(source),
        }
    }

    /// LSP position of a byte offset — the indexed equivalent of
    /// [`offset_to_position`].
    pub fn position(&self, offset: usize) -> Position {
        let (line, character) = self.index.utf16_line_col(self.source, offset);
        Position { line, character }
    }

    /// LSP range of a span — the indexed equivalent of [`span_to_range`].
    pub fn range(&self, span: Span) -> Range {
        Range {
            start: self.position(span.start),
            end: self.position(span.end),
        }
    }

    /// Position one past the end of the source (whole-document edits).
    pub fn end(&self) -> Position {
        self.position(self.source.len())
    }
}

/// Convert a byte offset into the source string into an LSP position.
pub fn offset_to_position(source: &str, offset: usize) -> Position {
    let mut line: u32 = 0;
    let mut column: u32 = 0;
    let bytes = source.as_bytes();
    let limit = offset.min(bytes.len());
    let mut i = 0;
    while i < limit {
        let b = bytes[i];
        if b == b'\n' {
            line += 1;
            column = 0;
            i += 1;
            continue;
        }
        // Move to next UTF-8 code point boundary.
        let cp_len = utf8_char_len(b);
        // LSP default encoding is UTF-16; count UTF-16 code units.
        // For ASCII (1 byte) and 2/3-byte UTF-8 (1 code unit) we increment
        // column by 1; for 4-byte UTF-8 (supplementary plane) it's 2 code
        // units.
        column += if cp_len == 4 { 2 } else { 1 };
        i += cp_len;
    }
    Position {
        line,
        character: column,
    }
}

/// Convert an LSP position into a byte offset. Returns None if the position
/// is past the end of the source.
pub fn position_to_offset(source: &str, position: Position) -> Option<usize> {
    let target_line = position.line;
    let target_char = position.character;
    let mut line: u32 = 0;
    let mut character: u32 = 0;
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if line == target_line && character == target_char {
            return Some(i);
        }
        let b = bytes[i];
        if b == b'\n' {
            if line == target_line {
                // Position is past end of this line; clamp to line end.
                return Some(i);
            }
            line += 1;
            character = 0;
            i += 1;
            continue;
        }
        let cp_len = utf8_char_len(b);
        character += if cp_len == 4 { 2 } else { 1 };
        i += cp_len;
    }
    if line == target_line && character >= target_char {
        Some(i)
    } else {
        None
    }
}

fn utf8_char_len(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first < 0xC0 {
        // Continuation byte; should not be the first byte of a char.
        1
    } else if first < 0xE0 {
        2
    } else if first < 0xF0 {
        3
    } else {
        4
    }
}

/// Convert a compiler [`Span`] into an LSP [`Range`].
pub fn span_to_range(source: &str, span: Span) -> Range {
    Range {
        start: offset_to_position(source, span.start),
        end: offset_to_position(source, span.end),
    }
}

/// The position one past the end of the source — used for "replace whole
/// document" formatting edits.
pub fn end_position(source: &str) -> Position {
    offset_to_position(source, source.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_offsets_match_columns() {
        let src = "abc\ndef";
        assert_eq!(offset_to_position(src, 0), Position::new(0, 0));
        assert_eq!(offset_to_position(src, 2), Position::new(0, 2));
        assert_eq!(offset_to_position(src, 4), Position::new(1, 0));
        assert_eq!(offset_to_position(src, 6), Position::new(1, 2));
    }

    #[test]
    fn position_round_trip() {
        let src = "alpha\n  beta\ngamma";
        let p = Position::new(1, 4);
        let off = position_to_offset(src, p).unwrap();
        assert_eq!(offset_to_position(src, off), p);
    }

    /// LSP characters are UTF-16 code units, not bytes: a 2-byte `é` and a
    /// 3-byte `€` each count 1, a 4-byte `🦀` counts 2. Cursor positions on
    /// lines with non-ASCII text before them must land on char boundaries.
    #[test]
    fn non_ascii_offsets_count_utf16_units() {
        // "-- café\nlet x" — é is 2 bytes / 1 UTF-16 unit.
        let src = "// café\nlet x";
        // After the é: 7 UTF-16 units into line 0, 8 bytes into the source.
        assert_eq!(position_to_offset(src, Position::new(0, 7)), Some(8));
        assert_eq!(offset_to_position(src, 8), Position::new(0, 7));
        // Next line is unaffected.
        assert_eq!(
            position_to_offset(src, Position::new(1, 3)),
            Some(src.find("let").unwrap() + 3)
        );

        // 4-byte astral char: 2 UTF-16 units.
        let crab = "🦀ab";
        assert_eq!(position_to_offset(crab, Position::new(0, 2)), Some(4));
        assert_eq!(position_to_offset(crab, Position::new(0, 3)), Some(5));
        assert_eq!(offset_to_position(crab, 4), Position::new(0, 2));
    }

    /// Every offset the converter returns is a char boundary — slicing the
    /// source at it can never panic.
    #[test]
    fn non_ascii_round_trips_on_char_boundaries() {
        let src = "π = 3\n// naïve café €10 🦀\nend";
        for line in 0..3u32 {
            for character in 0..24u32 {
                if let Some(off) = position_to_offset(src, Position::new(line, character)) {
                    assert!(
                        src.is_char_boundary(off),
                        "offset {off} for ({line},{character}) splits a codepoint"
                    );
                    let _ = &src[..off];
                }
            }
        }
    }
}
