// Unit coverage for the Test Explorer's coverage mapping (#2104): the pinned
// `bynk test --coverage --format json` document (its shape is
// `bynk-driver/src/test_json.rs`'s `Coverage`; the partial file's lines are the
// ones `bynkc/tests/coverage_behaviour.rs` pins for `coverage_partial`) mapped to
// per-file totals and per-line statements.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import { coverageFiles, coverageUnavailable } from "../../src/coverageModel";

const DOC = JSON.parse(
  fs.readFileSync(path.resolve(__dirname, "../../../test/fixtures/test-coverage.json"), "utf8"),
);
const ROOT = path.resolve("/work/shop");

describe("coverageFiles", () => {
  it("resolves each file against the run's project root, with its totals", () => {
    const files = coverageFiles(DOC.coverage, ROOT);
    assert.deepStrictEqual(
      files.map((f) => [f.file, f.covered, f.total]),
      [
        [path.join(ROOT, "src", "demo", "m.bynk"), 2, 5],
        [path.join(ROOT, "src", "window.bynk"), 8, 8],
      ],
    );
  });

  it("lists covered and uncovered lines, ascending", () => {
    const [partial, full] = coverageFiles(DOC.coverage, ROOT);
    assert.deepStrictEqual(partial.statements, [
      { line: 3, executed: true },
      { line: 4, executed: true },
      { line: 7, executed: false },
      { line: 8, executed: false },
      { line: 9, executed: false },
    ]);
    assert.ok(full.statements.every((s) => s.executed));
    assert.strictEqual(full.statements.length, 8);
  });

  it("marks only uncovered lines for a driver without covered_lines", () => {
    const [f] = coverageFiles(
      { files: [{ path: "src/a.bynk", covered: 1, lines: 3, uncovered: [5, 2] }] },
      ROOT,
    );
    assert.deepStrictEqual(f.statements, [
      { line: 2, executed: false },
      { line: 5, executed: false },
    ]);
  });

  it("is lenient about missing and malformed input", () => {
    assert.deepStrictEqual(coverageFiles(undefined, ROOT), []);
    assert.deepStrictEqual(coverageFiles({ files: "x" }, ROOT), []);
    const files = coverageFiles({ files: [null, { covered: 1 }, { path: "src/b.bynk", future: 1 }] }, ROOT);
    assert.strictEqual(files.length, 1);
    assert.deepStrictEqual([files[0].covered, files[0].total, files[0].statements], [0, 0, []]);
  });
});

describe("coverageUnavailable", () => {
  it("recognises the driver's coverage refusal", () => {
    const msg =
      "`--coverage` requires `tsc` and `node` on PATH (the CI-shaped path with `.js.map`s); the `tsx` fallback is not supported for coverage.";
    assert.strictEqual(coverageUnavailable(msg), msg);
    assert.strictEqual(coverageUnavailable("the runner exited with signal 9"), undefined);
    assert.strictEqual(coverageUnavailable(undefined), undefined);
  });
});
