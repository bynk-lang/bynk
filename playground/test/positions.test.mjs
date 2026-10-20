// Unit tests for src/positions.ts (#2126): UTF-16 <-> UTF-8 offset mapping at
// the CodeMirror / bynk-wasm boundary. Plain `node --test`; esbuild (already a
// devDependency) strips the TypeScript so no test framework is needed.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { transform } from "esbuild";

const source = await readFile(new URL("../src/positions.ts", import.meta.url), "utf8");
const { code } = await transform(source, { loader: "ts", format: "esm" });
const { utf16ToByteOffset, byteToUtf16Offset } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
);

const bytes = (s) => Buffer.byteLength(s, "utf8");

test("ASCII text maps one-to-one", () => {
  const text = "unit main\n";
  for (let i = 0; i <= text.length; i++) {
    assert.equal(utf16ToByteOffset(text, i), i);
    assert.equal(byteToUtf16Offset(text, i), i);
  }
});

test("offsets after a non-ASCII comment shift by the extra bytes", () => {
  // The issue's repro: `// café` above a declaration.
  const text = "// café\nlet x = 1\n";
  const at = text.indexOf("x");
  const byte = bytes(text.slice(0, at));
  assert.equal(byte, at + 1); // `é` is 2 bytes, 1 UTF-16 unit
  assert.equal(utf16ToByteOffset(text, at), byte);
  assert.equal(byteToUtf16Offset(text, byte), at);
});

test("3- and 4-byte characters (BMP and astral) round-trip", () => {
  const text = "a€b😀c";
  // UTF-16 index of each char start -> its byte offset.
  const pairs = [
    [0, 0], // a
    [1, 1], // €
    [2, 4], // b
    [3, 5], // 😀 (surrogate pair: UTF-16 3..4)
    [5, 9], // c
    [6, 10], // end
  ];
  for (const [u16, b] of pairs) {
    assert.equal(utf16ToByteOffset(text, u16), b, `utf16 ${u16}`);
    assert.equal(byteToUtf16Offset(text, b), u16, `byte ${b}`);
  }
});

test("a UTF-16 offset inside a surrogate pair snaps to the pair's start", () => {
  const text = "a😀b";
  assert.equal(utf16ToByteOffset(text, 2), utf16ToByteOffset(text, 1));
});

test("a byte offset inside a multi-byte character rounds down to its start", () => {
  const text = "a😀b";
  for (const b of [2, 3, 4]) assert.equal(byteToUtf16Offset(text, b), 1);
  const euro = "x€y";
  for (const b of [2, 3]) assert.equal(byteToUtf16Offset(euro, b), 1);
});

test("out-of-range offsets clamp to the text", () => {
  const text = "café";
  assert.equal(utf16ToByteOffset(text, -3), 0);
  assert.equal(utf16ToByteOffset(text, 99), bytes(text));
  assert.equal(byteToUtf16Offset(text, -3), 0);
  assert.equal(byteToUtf16Offset(text, 99), text.length);
  assert.equal(utf16ToByteOffset("", 5), 0);
  assert.equal(byteToUtf16Offset("", 5), 0);
});
