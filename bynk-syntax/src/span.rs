//! Source position spans.

/// T3.5 (R2.2): which file a `Span` belongs to. Allocated once per file by
/// the same "one counter, threaded from the per-project parse loop" shape
/// T3.4 used for `ExprId` (`phase_parse`/`parse_sources` in `bynk-emit`).
/// Defaults to [`FileId::UNKNOWN`] — most `Span` construction across the
/// workspace is either purely position-arithmetic (`merge`/`offset`, which
/// propagate whatever `file` the input spans already carried) or a
/// synthetic/single-file context (an LSP code action, a checker-internal
/// zero-width span) that was never at risk of the R2.2 defect (a *label*
/// rendered against the wrong file) in the first place — the defect is
/// specifically about a `Span` compared or rendered *across* files, and
/// those all originate at the lexer, the one place `FileId::UNKNOWN` is
/// never used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub u32);

impl FileId {
    pub const UNKNOWN: FileId = FileId(u32::MAX);
}

impl Default for FileId {
    fn default() -> Self {
        Self::UNKNOWN
    }
}

/// A byte range in the source. Half-open: `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Span {
    pub file: FileId,
    pub start: usize,
    pub end: usize,
}

impl Span {
    /// A `Span` with no real file identity — the default for every existing
    /// construction site the T3.5 migration didn't touch. See [`new_in`](Self::new_in)
    /// for the real-identity constructor the lexer uses.
    pub fn new(start: usize, end: usize) -> Self {
        Self {
            file: FileId::UNKNOWN,
            start,
            end,
        }
    }

    /// T3.5: a `Span` with a real file identity, attached at the one place
    /// (the lexer) where it's actually known.
    pub fn new_in(file: FileId, start: usize, end: usize) -> Self {
        Self { file, start, end }
    }

    pub fn range(&self) -> std::ops::Range<usize> {
        self.start..self.end
    }

    /// This span shifted right by `delta` bytes. Used to rebase spans produced
    /// against a substring (e.g. a re-lexed interpolation hole) into the full
    /// source. (#716.)
    pub fn offset(self, delta: usize) -> Span {
        Span {
            file: self.file,
            start: self.start + delta,
            end: self.end + delta,
        }
    }

    /// Span covering both `self` and `other` (the smallest enclosing range).
    /// T3.5: both operands are always the same file in practice (a merge
    /// never spans two files); `self`'s id wins over `other`'s `UNKNOWN` if
    /// only one side carries a real one, so a merge involving a genuinely
    /// lexer-sourced span doesn't lose its identity to a synthetic partner.
    pub fn merge(self, other: Span) -> Span {
        Span {
            file: if self.file != FileId::UNKNOWN {
                self.file
            } else {
                other.file
            },
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

#[cfg(test)]
mod default_tests {
    use super::{FileId, Span};

    /// R2.2: a `Span` built with no file identity (`Span::default()`, not
    /// `new_in`) must carry `FileId::UNKNOWN`, never `FileId(0)` — the id
    /// `parse_cache.rs` assigns to the first real file it interns.
    #[test]
    fn a_default_span_carries_no_file_identity() {
        assert_eq!(FileId::default(), FileId::UNKNOWN);
        assert_eq!(Span::default().file, FileId::UNKNOWN);
    }
}

impl From<std::ops::Range<usize>> for Span {
    fn from(r: std::ops::Range<usize>) -> Self {
        Span {
            file: FileId::UNKNOWN,
            start: r.start,
            end: r.end,
        }
    }
}

#[cfg(test)]
mod line_index_tests {
    use super::{LineIndex, line_col};

    /// `LineIndex::line_col` must agree with the scanning `line_col` at every
    /// offset, including past-the-end and non-ASCII sources.
    #[test]
    fn line_index_matches_scanning_line_col() {
        for src in [
            "",
            "abc",
            "abc\ndef",
            "abc\ndef\n",
            "\n\n\n",
            "π = 3\n// naïve café €10 🦀\nend",
        ] {
            let index = LineIndex::new(src);
            // Include one past-the-end offset to exercise the clamp.
            for offset in 0..=src.len() + 2 {
                if !src.is_char_boundary(offset.min(src.len())) {
                    continue;
                }
                assert_eq!(
                    index.line_col(src, offset),
                    line_col(src, offset),
                    "mismatch at offset {offset} in {src:?}",
                );
            }
        }
    }

    /// UTF-16 columns count code units: BMP chars are 1, astral chars 2. Line is
    /// 0-based and column resets to 0 after each newline.
    #[test]
    fn utf16_line_col_counts_code_units() {
        let src = "// café\nlet 🦀 x";
        let index = LineIndex::new(src);
        // After "café" on line 0: c,a,f + 2-byte é → 4 UTF-16 units.
        let after_cafe = "// café".len();
        assert_eq!(index.utf16_line_col(src, after_cafe), (0, 7));
        // Start of line 1.
        let line1 = src.find("let").unwrap();
        assert_eq!(index.utf16_line_col(src, line1), (1, 0));
        // Just past the 4-byte crab on line 1: "let " (4) + 🦀 (2 units).
        let after_crab = line1 + "let 🦀".len();
        assert_eq!(index.utf16_line_col(src, after_crab), (1, 6));
    }

    #[test]
    fn line_and_line_start_round_trip() {
        let src = "one\ntwo\nthree";
        let index = LineIndex::new(src);
        assert_eq!(index.line(0), 0);
        assert_eq!(index.line(3), 0); // the '\n' terminating line 0
        assert_eq!(index.line(4), 1); // start of "two"
        assert_eq!(index.line(src.len()), 2);
        assert_eq!(index.line_start(1), 4);
        assert_eq!(index.line_start(2), 8);
    }
}

/// 1-indexed (line, column) of a byte offset in `source`. Columns count
/// characters, not bytes. Lives in the syntax leaf so every layer that maps a
/// span to a position — the emitter's assertion locations, `bynkc`'s `short`
/// rendering, and (slice 6) `bynk-render` — shares one implementation.
///
/// This scans from byte 0, so it is O(offset). For repeated lookups over one
/// snapshot (an LSP request emitting many positions, or the emit source-map
/// builder resolving every checkpoint), build a [`LineIndex`] once and query
/// it in O(log n) instead — see #732.
pub fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// A per-snapshot table of line-start byte offsets, built once and shared by
/// every position lookup over that snapshot (#732).
///
/// `line_col` scans from byte 0 on every call, so emitting `n` positions over
/// an `n`-byte snapshot is O(n²). This precomputes the byte offset where each
/// line begins; a lookup binary-searches for the line (O(log n)) and then
/// counts columns only within that one line. Consumers that map many spans per
/// request — semantic tokens, folding ranges, diagnostics, inlay hints,
/// document symbols, the emit source map — build one of these per snapshot and
/// reuse it.
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Byte offset of the start of each line; `line_starts[0]` is always `0`.
    /// A trailing newline yields a final (empty) line start, matching the
    /// convention that offset == `len` after a `\n` sits on the next line.
    line_starts: Vec<usize>,
    /// Byte length of the indexed source, so out-of-range offsets clamp to the
    /// end exactly as the scanning `line_col` would.
    len: usize,
}

impl LineIndex {
    /// Precompute the line-start table for `source` in one O(n) pass.
    pub fn new(source: &str) -> Self {
        let mut line_starts = vec![0usize];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self {
            line_starts,
            len: source.len(),
        }
    }

    /// 0-based line containing `offset`, by binary search over the line starts.
    pub fn line(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            // `line_starts[0] == 0 <= offset`, so `Err(0)` is impossible and
            // `i - 1` never underflows.
            Err(i) => i - 1,
        }
    }

    /// Byte offset where the 0-based `line` begins.
    pub fn line_start(&self, line: usize) -> usize {
        self.line_starts[line]
    }

    /// 1-indexed (line, column) of `offset`, columns counting characters —
    /// identical to [`line_col`] but O(log n + line length) after the one-time
    /// build. `source` must be the same string the index was built from.
    pub fn line_col(&self, source: &str, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.len);
        let line = self.line(offset);
        let start = self.line_starts[line];
        let mut col = 1;
        for (i, _) in source[start..].char_indices() {
            if start + i >= offset {
                break;
            }
            col += 1;
        }
        (line + 1, col)
    }

    /// 0-based (line, UTF-16 column) of `offset` — the LSP default position
    /// encoding (columns count UTF-16 code units, so a 4-byte astral char is 2).
    /// `source` must be the same string the index was built from.
    pub fn utf16_line_col(&self, source: &str, offset: usize) -> (u32, u32) {
        let offset = offset.min(self.len);
        let line = self.line(offset);
        let start = self.line_starts[line];
        let mut col: u32 = 0;
        for (i, ch) in source[start..].char_indices() {
            if start + i >= offset {
                break;
            }
            col += ch.len_utf16() as u32;
        }
        (line as u32, col)
    }
}
