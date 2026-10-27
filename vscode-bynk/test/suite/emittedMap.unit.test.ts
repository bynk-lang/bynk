// #2471: the generated ↔ source line lookups, over a hand-built decoded map.
import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { readModules } from "../../src/emittedModules";
import {
  decodeMappings,
  decodeVlq,
  generatedRangeForSource,
  parseEmitted,
  sourceRangeForGenerated,
  type EmittedFile,
} from "../../src/emittedMap";

// Source lines: 0 `context`, 2 `service`, 3 `on call`, 4 `Ok(name)`, 5 `}`.
// Generated line 0-1 are unmapped; 2-3 are the header; 4-6 the body.
const file: EmittedFile = {
  path: "greet.ts",
  text: "",
  sources: ["/p/src/greet.bynk"],
  sourceIndex: 0,
  lines: [null, null, [0, 3], [0, 3], [0, 4], [0, 4], [0, 4]],
};
const noBlanks = () => false;

describe("emitted output line map", () => {
  it("maps a source line to its statement's generated run", () => {
    assert.deepStrictEqual(generatedRangeForSource(file, 4), { start: 4, end: 6 });
    assert.deepStrictEqual(generatedRangeForSource(file, 3), { start: 2, end: 3 });
  });

  it("lands blank space after a statement on that statement", () => {
    assert.deepStrictEqual(generatedRangeForSource(file, 5), { start: 4, end: 6 });
  });

  it("has nothing before the first mapped statement", () => {
    assert.strictEqual(generatedRangeForSource(file, 1), null);
  });

  it("ignores entries that map into another source", () => {
    const other: EmittedFile = { ...file, lines: [[1, 3], [0, 9]] };
    assert.deepStrictEqual(generatedRangeForSource(other, 9), { start: 1, end: 1 });
    assert.strictEqual(generatedRangeForSource(other, 3), null);
  });

  it("maps a generated line to its statement up to the next one", () => {
    assert.deepStrictEqual(sourceRangeForGenerated(file, 2, noBlanks), { start: 3, end: 3 });
    // Last statement: nothing follows, so just its own line.
    assert.deepStrictEqual(sourceRangeForGenerated(file, 5, noBlanks), { start: 4, end: 4 });
  });

  it("trims trailing blank source lines", () => {
    const gap: EmittedFile = { ...file, lines: [[0, 1], [0, 6]] };
    const blank = (l: number) => l >= 3 && l <= 5;
    assert.deepStrictEqual(sourceRangeForGenerated(gap, 0, blank), { start: 1, end: 2 });
  });

  it("has no source for an unmapped line or another source", () => {
    assert.strictEqual(sourceRangeForGenerated(file, 0, noBlanks), null);
    const other: EmittedFile = { ...file, lines: [[1, 2]] };
    assert.strictEqual(sourceRangeForGenerated(other, 0, noBlanks), null);
    assert.strictEqual(sourceRangeForGenerated(file, 99, noBlanks), null);
  });
});

describe("emitted output source maps", () => {
  it("decodes signed VLQ values", () => {
    assert.deepStrictEqual(decodeVlq("AAAA"), [0, 0, 0, 0]);
    assert.deepStrictEqual(decodeVlq("gBAC"), [16, 0, 1]);
    assert.deepStrictEqual(decodeVlq("DAAD"), [-1, 0, 0, -1]);
    assert.strictEqual(decodeVlq("!"), null);
  });

  it("accumulates source lines and keeps unmapped gaps", () => {
    // Line 0 unmapped; line 1 -> src 0 line 2; line 2 -> src 0 line 5.
    assert.deepStrictEqual(decodeMappings(";AAEA;AAGA"), [null, [0, 2], [0, 5]]);
  });

  it("uses the first segment of a line that has several", () => {
    assert.deepStrictEqual(decodeMappings("AAEA,CAAC;AAGA"), [[0, 2], [0, 5]]);
  });

  it("rejects a malformed mapping", () => {
    assert.strictEqual(decodeMappings("!"), null);
    assert.strictEqual(decodeMappings("AADA"), null); // line goes negative
  });

  const map = (sources: string[], mappings: string) => JSON.stringify({ version: 3, sources, mappings });

  it("finds the source among the map's sources", () => {
    const f = parseEmitted("a/greet.ts", "x", map(["/p/other.bynk", "/p/src/greet.bynk"], "ACAA"), "/p/src/greet.bynk");
    assert.strictEqual(f?.sourceIndex, 1);
    assert.deepStrictEqual(f?.lines, [[1, 0]]);
  });

  it("returns nothing for a module from other sources, or a broken map", () => {
    assert.strictEqual(parseEmitted("a.ts", "x", map(["/p/other.bynk"], "AAAA"), "/p/src/greet.bynk"), undefined);
    assert.strictEqual(parseEmitted("a.ts", "x", "not json", "/p/src/greet.bynk"), undefined);
    assert.strictEqual(parseEmitted("a.ts", "x", map(["/p/src/greet.bynk"], "!"), "/p/src/greet.bynk"), undefined);
  });

  it("reads the modules a file generates, in path order, and never the runtime", () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "bynk-emitted-test-"));
    try {
      const src = "/p/src/greet.bynk";
      const put = (rel: string, text: string, mappedTo?: string) => {
        fs.mkdirSync(path.dirname(path.join(dir, rel)), { recursive: true });
        fs.writeFileSync(path.join(dir, rel), text);
        if (mappedTo) fs.writeFileSync(path.join(dir, `${rel}.map`), map([mappedTo], "AAAA"));
      };
      put("workers/greet/handlers.ts", "// handlers\n", src);
      put("workers/greet/index.ts", "// entry\n", src);
      put("workers/other/index.ts", "// not ours\n", "/p/src/other.bynk");
      put("__bynk_runtime/result.ts", "// runtime\n"); // no map
      const files = readModules(dir, src);
      assert.deepStrictEqual(files.map((f) => f.path), ["workers/greet/handlers.ts", "workers/greet/index.ts"]);
      assert.ok(files.every((f) => f.sourceIndex === 0));
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  it("highlights only the first run of a statement whose lines are split", () => {
    // A known limit: a statement mapped to two separate blocks gets the first.
    const split: EmittedFile = { path: "x.ts", text: "", sources: ["s"], sourceIndex: 0, lines: [[0, 3], [0, 4], [0, 3]] };
    assert.deepStrictEqual(generatedRangeForSource(split, 3), { start: 0, end: 0 });
  });
});
