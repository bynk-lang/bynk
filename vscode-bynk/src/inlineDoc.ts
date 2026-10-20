// #849: in-editor doc-comment rendering — the pure Markdown-affordance tokenizer.
//
// Doc comments in Bynk are `--- … ---` blocks (bynk-syntax/src/lexer.rs) whose
// body is Markdown, but in the source buffer they render as flat, comment-
// coloured text. This module turns each block's body into a flat list of
// decoration ranges — heading lines, `**strong**` spans, `*emphasis*` spans —
// which the extension layers over the buffer as `TextEditorDecorationType`s
// (see `registerInlineDocRendering` in extension.ts).
//
// Design (per the #849 proposal's DECISIONS):
//   * [A] Style-in-place, not conceal-and-reveal: the `#`/`**`/`*` markers stay
//     visible; we only colour/weight/italicise the span they mark. Robust —
//     never fights selection, copy, or multi-cursor.
//   * [B] Direct decorations, not semantic tokens: bold must *look* bold, which
//     a theme-driven semantic token cannot guarantee. Ranges here map to
//     `fontWeight`/`fontStyle`, not token types.
//   * [C] Client-parse: this small self-contained tokenizer lives in the
//     extension rather than a new `bynk/docDecorations` LSP verb, since inline
//     rendering is a pure client concern. It is deliberately independent of the
//     server so it needs no round-trip on every keystroke.
//
// Clickable `[Name]`/`[Owner.member]` links are NOT produced here — they flow
// through the server's `textDocument/documentLink` (#848), so the extension
// needs no client-side link work.
//
// This is a pure function of the document text so it is unit-testable without a
// live editor (VS Code exposes no API to read back applied decorations); see
// test/suite/inlineDoc.unit.test.ts. All offsets are JS string indices, i.e.
// UTF-16 code units — exactly what `vscode.Position.character` counts — so a
// doc block with multi-byte content (emoji, non-Latin text) maps correctly
// without any byte↔UTF-16 conversion (the classic off-by-N decoration bug we
// sidestep by scanning client-side rather than consuming Rust byte spans).

/** The kind of Markdown affordance a range carries — one decoration type each. */
export type DocDecorationKind = "heading" | "strong" | "emphasis";

/** A half-open range within a single line, in `(line, character)` terms.
 *  Every affordance we render is line-local, so `line` is shared by both ends. */
export interface DocDecorationRange {
  kind: DocDecorationKind;
  line: number;
  /** Inclusive start column (UTF-16 code units). */
  startChar: number;
  /** Exclusive end column. */
  endChar: number;
}

// A doc-block marker line: optional leading whitespace, three-or-more hyphens,
// optional trailing whitespace, end of line. Mirrors `doc_block_open_at` in
// bynk-syntax/src/lexer.rs (which also permits ≥3 hyphens and surrounding
// horizontal whitespace). Trailing `\r` is tolerated for CRLF sources. Group 1
// is the dashes: a block closes only on a marker of its opener's length (#1885).
const MARKER = /^[ \t]*(-{3,})[ \t]*\r?$/;

// #2203: a `--- text` one-line doc: three or more hyphens first on the line
// (after optional horizontal whitespace), at least one space or tab, then text.
// Group 1 is all of that prefix and group 2 the marker; the rest of the line is
// the doc's Markdown, less an optional closer (a final run of exactly the
// marker's hyphens after whitespace). Mirrors the lexer's `DocLine` rule. A
// marker alone on its line is a block fence, which `MARKER` takes first.
const DOC_LINE = /^([ \t]*(-{3,})[ \t]+)(?=\S)/;

// An ATX heading line: optional leading whitespace, 1–6 `#`, then whitespace or
// end-of-line. `#Heading` (no space) is not a heading, matching CommonMark.
const HEADING = /^([ \t]*)(#{1,6})(?=[ \t]|\r?$)/;

/**
 * Tokenize every `--- … ---` doc block, and every `--- text` doc line (#2203), in
 * `text` into decoration ranges.
 *
 * Blocks are paired by scanning lines top-to-bottom: a marker line opens a
 * block, the next marker line of the same length closes it, and scanning
 * resumes after the close — the same pairing the lexer's `doc_block_close`
 * performs (#1885). A marker of another length inside a block is body text. A
 * final unclosed block yields no decorations (its body is a lex error anyway).
 *
 * Marker lines themselves are never decorated. Within a block body, each line is
 * either a heading (whole line coloured) or scanned for inline emphasis.
 */
export function docDecorations(text: string): DocDecorationRange[] {
  const ranges: DocDecorationRange[] = [];
  // Split on `\n` only so a trailing `\r` stays attached to the line; content
  // columns are unaffected because `\r` only ever sits at end-of-line, past any
  // affordance. Line numbers are the split index — VS Code's own line numbering.
  const lines = text.split("\n");
  // The open block's opener: its dash count and line, or `null` outside one.
  let open: { dashes: number; line: number } | null = null;

  for (let line = 0; line < lines.length; line++) {
    const raw = lines[line];
    const marker = MARKER.exec(raw);
    if (marker && open === null) {
      open = { dashes: marker[1].length, line };
      continue;
    }
    if (marker && open !== null && marker[1].length === open.dashes) {
      // A closing marker is never itself decorated.
      open = null;
      continue;
    }
    if (open === null) {
      // #2203: outside a block, a `--- text` line's text is a doc too. Blank
      // its prefix to spaces so the heading/emphasis columns stay the line's
      // own, and drop its closer, which is not text.
      const doc = DOC_LINE.exec(raw);
      if (doc) {
        let body = raw.slice(doc[1].length);
        const closer = new RegExp(`[ \t]+${doc[2]}[ \t]*\r?$`).exec(body);
        if (closer) {
          body = body.slice(0, closer.index);
        }
        decorateLine(" ".repeat(doc[1].length) + body, line, ranges);
      }
      continue;
    }
    decorateLine(raw, line, ranges);
  }

  // The last block never closed. The lexer treats that as an error; we simply
  // drop what we speculatively decorated inside it so half a document doesn't
  // light up while the writer is mid-block.
  if (open !== null) {
    const opened = open.line;
    return ranges.filter((r) => r.line < opened);
  }
  return ranges;
}

/** Decorate one body line: a heading colours the whole line; otherwise scan for
 *  inline `**strong**` / `*emphasis*` runs. */
function decorateLine(
  raw: string,
  line: number,
  out: DocDecorationRange[],
): void {
  // Strip a trailing CR so the heading/emphasis range ends at the last visible
  // character, not the invisible `\r`.
  const content = raw.endsWith("\r") ? raw.slice(0, -1) : raw;

  const heading = HEADING.exec(content);
  if (heading) {
    // Colour from the first `#` to end of line (the leading whitespace, group 1,
    // is excluded). Headings own their line — no inline scan on top, keeping the
    // affordance unambiguous.
    out.push({
      kind: "heading",
      line,
      startChar: heading[1].length,
      endChar: content.length,
    });
    return;
  }

  scanEmphasis(content, line, out);
}

/**
 * Scan a single line for `**strong**`/`__strong__` and `*emphasis*`/`_emphasis_`
 * runs, left to right, non-overlapping.
 *
 * Guards against the common false positives:
 *   * a run's content must be non-empty and must not touch whitespace at either
 *     inner edge (`a * b * c` is arithmetic-looking prose, not emphasis);
 *   * `_` delimiters additionally require a word boundary on the outside, so a
 *     `snake_case` identifier (or `a_b_c`) inside a doc comment is never
 *     italicised. `*` has no such intraword rule (CommonMark treats `*` as
 *     always-flanking), and code identifiers rarely embed literal asterisks.
 */
function scanEmphasis(
  s: string,
  line: number,
  out: DocDecorationRange[],
): void {
  let i = 0;
  while (i < s.length) {
    const ch = s[i];
    if (ch !== "*" && ch !== "_") {
      i++;
      continue;
    }
    const isDouble = s[i + 1] === ch;
    const delimLen = isDouble ? 2 : 1;
    const kind: DocDecorationKind = isDouble ? "strong" : "emphasis";

    // `_` only opens on a left word boundary (start, or a non-word char before).
    if (ch === "_" && !leftFlank(s, i)) {
      i++;
      continue;
    }

    const contentStart = i + delimLen;
    const close = findClose(s, contentStart, ch, delimLen);
    if (close < 0) {
      // No valid closer on this line — skip just this delimiter char and keep
      // scanning (a later, well-formed run on the same line still renders).
      i++;
      continue;
    }

    out.push({ kind, line, startChar: i, endChar: close + delimLen });
    i = close + delimLen; // resume past the closer; runs never overlap.
  }
}

/** Find the closing delimiter for a run opened at `contentStart` with `delim`
 *  (`ch` repeated `delimLen` times). Returns the index of the closer's first
 *  char, or -1 if there is no valid, non-empty, non-whitespace-hugging run. */
function findClose(
  s: string,
  contentStart: number,
  ch: string,
  delimLen: number,
): number {
  // Content may not start with whitespace (`** bold**` is not strong).
  if (contentStart >= s.length || isSpace(s[contentStart])) return -1;

  for (let j = contentStart; j <= s.length - delimLen; j++) {
    if (s[j] !== ch) continue;
    // Match exactly `delimLen` of `ch`: for a single `*`, the next char must not
    // also be `*` (that would be a `**` boundary, not our closer); for `**`,
    // require two in a row.
    if (delimLen === 1) {
      if (s[j + 1] === ch) {
        j++; // step over the `**` pair so we don't treat its first char as close
        continue;
      }
    } else if (s[j + 1] !== ch) {
      continue;
    }
    const contentEnd = j; // exclusive
    if (contentEnd <= contentStart) return -1; // empty run (`****`, `__`)
    if (isSpace(s[contentEnd - 1])) return -1; // closer hugs whitespace
    // `_` needs a right word boundary just past the closer.
    if (ch === "_" && !rightFlank(s, j + delimLen)) {
      continue;
    }
    return j;
  }
  return -1;
}

function isSpace(c: string): boolean {
  return c === " " || c === "\t";
}

/** True at a word char (for `_`'s intraword guard). */
function isWord(c: string | undefined): boolean {
  return c !== undefined && /\w/.test(c);
}

/** A `_` at `i` may open only if the char before it is not a word char, i.e. it
 *  is at line start or follows whitespace/punctuation. */
function leftFlank(s: string, i: number): boolean {
  return !isWord(s[i - 1]);
}

/** A `_` closer is valid only if the char after the run is not a word char. */
function rightFlank(s: string, after: number): boolean {
  return !isWord(s[after]);
}
