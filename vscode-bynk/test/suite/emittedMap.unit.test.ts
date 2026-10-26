// #2471: the generated ↔ source line lookups, over a hand-built decoded map.
import * as assert from "assert";

import {
  generatedRangeForSource,
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
