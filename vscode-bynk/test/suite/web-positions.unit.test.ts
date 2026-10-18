// Unit coverage for the web worker's LSP ↔ bynk-wasm position mapping (#2102).
// bynk-wasm speaks UTF-8 byte offsets, LSP speaks UTF-16 `(line, character)`;
// a byte offset inside a multi-byte character traps `bynk_complete`.

import * as assert from "assert";

import { LineIndex } from "../../src/web/positions";

describe("web LineIndex", () => {
  it("maps ASCII positions to the same byte offsets", () => {
    const index = new LineIndex("ab\ncd\n");
    assert.strictEqual(index.byteOffsetAt({ line: 0, character: 1 }), 1);
    assert.strictEqual(index.byteOffsetAt({ line: 1, character: 0 }), 3);
    assert.strictEqual(index.byteOffsetAt({ line: 1, character: 2 }), 5);
    assert.deepStrictEqual(index.positionOf(4), { line: 1, character: 1 });
  });

  it("counts multi-byte characters as UTF-8 bytes on the wasm side", () => {
    // `é` is 2 UTF-8 bytes / 1 UTF-16 unit; `😀` is 4 bytes / 2 units.
    const text = 'let s = "é😀"\nx';
    const index = new LineIndex(text);
    // After `é`: UTF-16 character 10, byte 11.
    assert.strictEqual(index.byteOffsetAt({ line: 0, character: 10 }), 11);
    // After `😀`: UTF-16 character 12, byte 15.
    assert.strictEqual(index.byteOffsetAt({ line: 0, character: 12 }), 15);
    // Line 1 starts after the closing quote and newline: byte 17.
    assert.strictEqual(index.byteOffsetAt({ line: 1, character: 0 }), 17);
    assert.deepStrictEqual(index.positionOf(17), { line: 1, character: 0 });
    assert.deepStrictEqual(index.positionOf(15), { line: 0, character: 12 });
  });

  it("never yields a byte offset inside a character", () => {
    const index = new LineIndex("😀x");
    // Between the two halves of the surrogate pair: snaps back to the start.
    assert.strictEqual(index.byteOffsetAt({ line: 0, character: 1 }), 0);
    assert.strictEqual(index.byteOffsetAt({ line: 0, character: 2 }), 4);
    // A byte offset inside the emoji rounds down to its start.
    assert.deepStrictEqual(index.positionOf(2), { line: 0, character: 0 });
  });

  it("clamps positions past the line or the text", () => {
    const index = new LineIndex("ab\ncd");
    // Past the end of line 0 clamps to the start of line 1 (the newline's end).
    assert.strictEqual(index.byteOffsetAt({ line: 0, character: 99 }), 3);
    assert.strictEqual(index.byteOffsetAt({ line: 9, character: 0 }), 5);
    assert.strictEqual(index.byteOffsetAt({ line: -1, character: 0 }), 0);
    assert.deepStrictEqual(index.positionOf(999), { line: 1, character: 2 });
    assert.deepStrictEqual(index.positionOf(-5), { line: 0, character: 0 });
  });
});
