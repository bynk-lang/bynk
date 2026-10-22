// #2104 integration test: the Testing view's coverage profile runs
// `bynk test --coverage --format json` against the project root. Runs against
// the stand-in `bynk` (test/fixtures/fake-bynk), which prints the pinned
// coverage document.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
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

// Test Explorer depth track, Slice E (#2269): Run with Coverage on one case measures
// that case alone, and on anything wider measures the project. The run is named for
// the case and says what it measured, which a test cannot read back from the
// extension host; the plan behind both is unit-tested (`coverageScope`).
describe("Bynk single-case coverage (#2269)", () => {
  const textFile = vscode.Uri.file(path.resolve(__dirname, "../../../test/fixtures/workspace/src/text.bynk"));
  let dir: string;
  let fake: Awaited<ReturnType<typeof useFakeBynk>>;

  before(async () => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), "bynk-case-coverage-"));
    fake = await useFakeBynk();
    // Two cases, both declared in text.bynk, on its first two lines.
    const doc = {
      passed: 2,
      failed: 0,
      suites: [
        {
          name: "text",
          kind: "unit",
          cases: ["cover one", "cover two"].map((name, i) => ({
            name,
            outcome: "pass",
            location: { path: "src/text.bynk", line: i + 1, col: 1 },
          })),
        },
      ],
    };
    const file = path.join(dir, "doc.json");
    fs.writeFileSync(file, JSON.stringify(doc));
    process.env.BYNK_FAKE_TEST_DOC = file;
    await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(textFile));
    await vscode.commands.executeCommand("testing.refreshTests");
    const deadline = Date.now() + 45_000;
    for (;;) {
      const lenses =
        (await vscode.commands.executeCommand<vscode.CodeLens[]>("vscode.executeCodeLensProvider", textFile)) ?? [];
      const names = lenses.filter((l) => l.command?.command === "bynk.runTestCase").map((l) => l.command?.arguments?.[0]);
      if (names.includes("cover one") && names.includes("cover two")) break;
      if (Date.now() > deadline) throw new Error(`the tree never held both cases; lenses: ${names}`);
      await new Promise((r) => setTimeout(r, 400));
    }
  });

  after(async () => {
    await fake.restore();
    delete process.env.BYNK_FAKE_TEST_DOC;
    fs.rmSync(dir, { recursive: true, force: true });
  });

  /** The coverage runs logged since entry `from`. */
  const coverageRuns = (from: number): string[][] =>
    fake
      .read()
      .slice(from)
      .map((e) => e.argv ?? [])
      .filter((a) => a[0] === "test" && a.includes("--coverage"));

  it("runs one case alone with --case and --coverage", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(60_000);
    const from = fake.read().length;
    const editor = await vscode.window.showTextDocument(textFile);
    editor.selection = new vscode.Selection(0, 0, 0, 0);
    await vscode.commands.executeCommand("testing.coverageAtCursor");
    await waitFor(() => coverageRuns(from).length >= 1, "the case's coverage run");
    const args = coverageRuns(from)[0];
    assert.strictEqual(args[args.indexOf("--case") + 1], "cover one", `the case under the cursor: ${args}`);
  });

  it("runs the project, not a --case, for a wider selection", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(60_000);
    const from = fake.read().length;
    await vscode.window.showTextDocument(textFile);
    await vscode.commands.executeCommand("testing.coverageCurrentFile");
    await waitFor(() => coverageRuns(from).length >= 1, "the file's coverage run");
    const args = coverageRuns(from)[0];
    assert.ok(!args.includes("--case") && !args.includes("--suite"), `the whole project: ${args}`);
  });
});
