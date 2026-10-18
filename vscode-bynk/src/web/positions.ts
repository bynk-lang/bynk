// Position mapping between LSP and `bynk-wasm` (#2102).
//
// `bynk-wasm` speaks UTF-8 byte offsets: `bynk_hover`/`bynk_complete` take one,
// and `bynk_analyze` reports each diagnostic's span as byte `from`/`to`. LSP
// speaks `(line, character)` with `character` in UTF-16 code units. A wrong
// mapping is not just a misplaced squiggle: `bynk_complete` slices the source at
// the offset it is given, so an offset inside a multi-byte character traps the
// wasm instance. Everything here clamps and snaps to a character boundary.
//
// Dependency-free (no `vscode`, no LSP packages) so the unit tests compile it
// directly (tsconfig.test.json).

export interface Position {
  line: number;
  character: number;
}

/** The UTF-8 length of one UTF-16 code point starting at `code`. */
function utf8Len(code: number): number {
  if (code < 0x80) return 1;
  if (code < 0x800) return 2;
  if (code < 0x10000) return 3;
  return 4;
}

/**
 * A source line table: converts between UTF-8 byte offsets and LSP positions.
 * Built once per document version and reused for every diagnostic.
 */
export class LineIndex {
  /** UTF-16 offset of the start of each line. */
  private readonly lineStarts: number[];

  constructor(private readonly text: string) {
    this.lineStarts = [0];
    for (let i = 0; i < text.length; i++) {
      if (text.charCodeAt(i) === 0x0a) this.lineStarts.push(i + 1);
    }
  }

  /**
   * The UTF-8 byte offset of a UTF-16 offset into the text. Clamped to
   * `[0, text.length]`; an offset between the two halves of a surrogate pair
   * snaps back to the start of the pair, so the result is always a character
   * boundary.
   */
  byteOffsetOf(utf16Offset: number): number {
    let end = Math.max(0, Math.min(Math.floor(utf16Offset), this.text.length));
    if (end > 0 && end < this.text.length && isLowSurrogate(this.text.charCodeAt(end))) {
      end -= 1;
    }
    let bytes = 0;
    for (let i = 0; i < end; ) {
      const code = this.text.codePointAt(i) ?? 0;
      bytes += utf8Len(code);
      i += code > 0xffff ? 2 : 1;
    }
    return bytes;
  }

  /** The UTF-8 byte offset of an LSP position (clamped to the text). */
  byteOffsetAt(pos: Position): number {
    return this.byteOffsetOf(this.utf16OffsetAt(pos));
  }

  /**
   * The UTF-16 offset of an LSP position. A `character` past the end of its
   * line clamps to the end of that line, before its `\n` or `\r\n` (LSP
   * semantics), never onto the next line.
   */
  utf16OffsetAt(pos: Position): number {
    if (pos.line < 0) return 0;
    if (pos.line >= this.lineStarts.length) return this.text.length;
    const start = this.lineStarts[pos.line];
    let end =
      pos.line + 1 < this.lineStarts.length ? this.lineStarts[pos.line + 1] - 1 : this.text.length;
    if (end > start && this.text.charCodeAt(end - 1) === 0x0d) end -= 1;
    return Math.max(start, Math.min(start + Math.max(0, pos.character), end));
  }

  /**
   * The UTF-16 offset of a UTF-8 byte offset. A byte offset past the end clamps
   * to the end; one inside a multi-byte character rounds down to its start.
   */
  utf16OffsetOf(byteOffset: number): number {
    const target = Math.max(0, Math.floor(byteOffset));
    let bytes = 0;
    let i = 0;
    while (i < this.text.length) {
      const code = this.text.codePointAt(i) ?? 0;
      const len = utf8Len(code);
      if (bytes + len > target) break;
      bytes += len;
      i += code > 0xffff ? 2 : 1;
    }
    return i;
  }

  /** The LSP position of a UTF-8 byte offset. */
  positionOf(byteOffset: number): Position {
    const offset = this.utf16OffsetOf(byteOffset);
    // The last line start at or before `offset`.
    let lo = 0;
    let hi = this.lineStarts.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (this.lineStarts[mid] <= offset) lo = mid;
      else hi = mid - 1;
    }
    return { line: lo, character: offset - this.lineStarts[lo] };
  }
}

function isLowSurrogate(code: number): boolean {
  return code >= 0xdc00 && code <= 0xdfff;
}
