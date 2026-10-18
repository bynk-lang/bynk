// Position mapping between CodeMirror and `bynk-wasm` (#2126).
//
// `bynk-wasm` speaks UTF-8 byte offsets: `bynk_hover`/`bynk_complete` take one,
// and `bynk_analyze` reports each diagnostic's span as byte `from`/`to`.
// CodeMirror positions are UTF-16 code-unit offsets into the document. The two
// agree only while everything before the position is ASCII. A wrong mapping is
// not just a misplaced squiggle: `bynk_complete` slices the source at the offset
// it is given, so an offset inside a multi-byte character would trap the wasm
// instance. Everything here clamps and snaps to a character boundary.
//
// The same approach as the VS Code web extension's `positions.ts` (#2119), minus
// the line/character layer CodeMirror doesn't need. Dependency-free and pure so
// `test/positions.test.mjs` can run it under plain `node --test`.

/** The UTF-8 length of the code point `code`. */
function utf8Len(code: number): number {
  if (code < 0x80) return 1;
  if (code < 0x800) return 2;
  if (code < 0x10000) return 3;
  return 4;
}

function isLowSurrogate(code: number): boolean {
  return code >= 0xdc00 && code <= 0xdfff;
}

/**
 * The UTF-8 byte offset of a UTF-16 offset into `text`. Clamped to
 * `[0, text.length]`; an offset between the two halves of a surrogate pair snaps
 * back to the start of the pair, so the result is always a character boundary.
 */
export function utf16ToByteOffset(text: string, utf16Offset: number): number {
  let end = Math.max(0, Math.min(Math.floor(utf16Offset), text.length));
  if (end > 0 && end < text.length && isLowSurrogate(text.charCodeAt(end))) {
    end -= 1;
  }
  let bytes = 0;
  for (let i = 0; i < end; ) {
    const code = text.codePointAt(i) ?? 0;
    bytes += utf8Len(code);
    i += code > 0xffff ? 2 : 1;
  }
  return bytes;
}

/**
 * The UTF-16 offset of a UTF-8 byte offset into `text`. A byte offset past the
 * end clamps to the end; a negative one clamps to 0; one inside a multi-byte
 * character rounds down to that character's start.
 */
export function byteToUtf16Offset(text: string, byteOffset: number): number {
  const target = Math.max(0, Math.floor(byteOffset));
  let bytes = 0;
  let i = 0;
  while (i < text.length) {
    const code = text.codePointAt(i) ?? 0;
    const len = utf8Len(code);
    if (bytes + len > target) break;
    bytes += len;
    i += code > 0xffff ? 2 : 1;
  }
  return i;
}
