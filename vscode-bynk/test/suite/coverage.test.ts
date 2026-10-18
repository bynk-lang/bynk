// #2104 integration test: the Testing view's coverage profile runs
// `bynk test --coverage --format json` against the project root. Runs against
// the stand-in `bynk` (test/fixtures/fake-bynk), which prints the pinned
// coverage document.

import * as assert from "assert";
import * as path from "path";
import * as vscode from "vscode";

import { useFakeBynk, waitFor } from "./fakeBynk";

describe("Bynk test coverage (#2104)", () => {
  let fake: Awaited<ReturnType<typeof useFakeBynk>>;

  before(async () => {
    fake = await useFakeBynk();
  });

  after(async () => {
    await fake.restore();
  });

  it("Run with Coverage passes --coverage and the absolute project root", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(30_000);
    await vscode.commands.executeCommand("testing.coverageAll");
    await waitFor(
      () => fake.read().some((e) => e.argv?.[0] === "test" && e.argv.includes("--coverage")),
      "a coverage run",
    );
    const run = fake.read().find((e) => e.argv?.[0] === "test" && e.argv.includes("--coverage"))!;
    assert.deepStrictEqual(run.argv!.slice(2), ["--format", "json", "--coverage"]);
    assert.ok(
      path.isAbsolute(run.argv![1]) && path.basename(run.argv![1]) === "workspace",
      `the project is named by its absolute root, got ${run.argv![1]}`,
    );
  });
});
