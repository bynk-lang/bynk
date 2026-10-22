// Run the system tests against a target (Test Explorer depth track, Slice B),
// inside the extension host and against the stand-in `bynk` (#2094): which
// target the quick pick offers, what the extension passes the driver, which
// cases it reports, and what it does with a refusal.
//
// The stand-in serves a document shaped by the driver's own (`test_json.rs`),
// with the per-case `tier` the driver attaches; it keeps only the `system`
// cases for `--against`, as the driver does. Nothing here touches a deployment.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { applyDocument, type RunSink } from "../../src/testing";
import { caseKey, planRun, systemOnly, type PlanCase, type TestRun } from "../../src/testPlan";
import { FAKE_BYNK, useFakeBynk, waitFor, type FakeLogEntry } from "./fakeBynk";

const FIXTURES = path.resolve(__dirname, "../../../test/fixtures");

describe("Run system tests against a target", () => {
  const workspace = path.resolve(FIXTURES, "workspace");
  const textFile = vscode.Uri.file(path.join(workspace, "src", "text.bynk"));
  const manifest = path.join(workspace, "bynk.toml");
  let dir: string;
  let fake: Awaited<ReturnType<typeof useFakeBynk>>;
  let originalManifest: string;
  let showQuickPick: typeof vscode.window.showQuickPick;
  let showWarningMessage: typeof vscode.window.showWarningMessage;
  /** The labels the last quick pick offered. */
  let offered: string[] = [];

  const CASES = [
    ["a unit case", "unit"],
    ["a wired case", "system"],
    ["another wired case", "system"],
    ["an integration case", "integration"],
  ] as const;

  function writeDoc(): string {
    const doc = {
      passed: CASES.length,
      failed: 0,
      suites: [
        {
          name: "text",
          kind: "unit",
          cases: CASES.map(([name, tier], i) => ({
            name,
            outcome: "pass",
            tier,
            location: { path: "src/text.bynk", line: i + 1, col: 1 },
          })),
        },
      ],
    };
    const file = path.join(dir, "doc-tiers.json");
    fs.writeFileSync(file, JSON.stringify(doc));
    return file;
  }

  before(async function () {
    this.timeout(90_000);
    dir = fs.mkdtempSync(path.join(os.tmpdir(), "bynk-against-"));
    fs.chmodSync(FAKE_BYNK, 0o755);
    fake = await useFakeBynk();
    // The workspace manifest gains two targets for these tests, and is put back.
    originalManifest = fs.readFileSync(manifest, "utf8");
    fs.writeFileSync(
      manifest,
      originalManifest +
        '\n[test.targets.preview]\nurl = "https://preview.example.com"\n\n[test.targets."staging eu"]\nurl = "https://eu.example.com"\n',
    );
    process.env.BYNK_FAKE_TEST_DOC = writeDoc();
    await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(textFile));
    await vscode.commands.executeCommand("testing.refreshTests");
    const deadline = Date.now() + 45_000;
    for (;;) {
      const lenses =
        (await vscode.commands.executeCommand<vscode.CodeLens[]>("vscode.executeCodeLensProvider", textFile)) ?? [];
      const names = lenses.filter((l) => l.command?.command === "bynk.runTestCase").map((l) => l.command?.arguments?.[0]);
      if (CASES.every(([n]) => names.includes(n))) break;
      if (Date.now() > deadline) throw new Error(`the tree never held the cases; lenses: ${names}`);
      await new Promise((r) => setTimeout(r, 400));
    }
    showQuickPick = vscode.window.showQuickPick;
    showWarningMessage = vscode.window.showWarningMessage;
  });

  after(async () => {
    (vscode.window as { showQuickPick: unknown }).showQuickPick = showQuickPick;
    (vscode.window as { showWarningMessage: unknown }).showWarningMessage = showWarningMessage;
    fs.writeFileSync(manifest, originalManifest);
    delete process.env.BYNK_FAKE_TEST_DOC;
    delete process.env.BYNK_FAKE_AGAINST;
    await fake.restore();
    fs.rmSync(dir, { recursive: true, force: true });
  });

  /** Make the quick pick choose `label`, recording what it offered. */
  function choose(label: string): void {
    (vscode.window as { showQuickPick: unknown }).showQuickPick = async (items: readonly vscode.QuickPickItem[]) => {
      offered = items.map((i) => i.label);
      return items.find((i) => i.label === label);
    };
  }

  function runs(from: number): string[][] {
    return fake
      .read()
      .slice(from)
      .map((e: FakeLogEntry) => e.argv ?? [])
      .filter((a) => a[0] === "test" && !a.includes("--no-run"));
  }

  it("offers dev and the manifest's targets, and passes the chosen name to the driver", async function () {
    this.timeout(60_000);
    const from = fake.read().length;
    choose("staging eu");
    await vscode.commands.executeCommand("bynk.runTestsAgainst");
    await waitFor(() => runs(from).length > 0, "a run against a target");

    assert.deepStrictEqual(offered.slice(0, 3), ["dev", "preview", "staging eu"]);
    assert.ok(offered.includes("Enter a target name…"), `a manual entry is offered: ${offered}`);
    const ran = runs(from);
    assert.strictEqual(ran.length, 1, `one process, got ${JSON.stringify(ran)}`);
    assert.deepStrictEqual(ran[0].slice(ran[0].indexOf("--against")), ["--against", "staging eu"]);
    assert.ok(!ran[0].includes("--skip-unsupported"), "skipping unsupported cases is never the default");
    assert.ok(!ran[0].includes("--coverage") && !ran[0].includes("--inspect"));
  });

  it("remembers the last target and offers it first next time", async function () {
    this.timeout(60_000);
    const from = fake.read().length;
    choose("preview");
    await vscode.commands.executeCommand("bynk.runTestsAgainst");
    await waitFor(() => runs(from).length > 0, "a run against preview");
    choose("preview");
    const again = fake.read().length;
    await vscode.commands.executeCommand("bynk.runTestsAgainst");
    await waitFor(() => runs(again).length > 0, "a second run against preview");
    assert.strictEqual(offered[0], "preview", `the last choice leads: ${offered}`);
  });

  it("never offers dev twice, or a URL", async function () {
    this.timeout(60_000);
    const from = fake.read().length;
    choose("dev");
    await vscode.commands.executeCommand("bynk.runTestsAgainst");
    await waitFor(() => runs(from).length > 0, "a run against dev");
    assert.strictEqual(offered.filter((l) => l === "dev").length, 1);
    assert.ok(!offered.some((l) => /:\/\//.test(l)));
  });

  it("shows a driver refusal rather than a crash", async function () {
    this.timeout(60_000);
    process.env.BYNK_FAKE_AGAINST = "refused";
    try {
      const from = fake.read().length;
      choose("preview");
      await vscode.commands.executeCommand("bynk.runTestsAgainst");
      await waitFor(() => runs(from).length > 0, "a refused run");
      assert.strictEqual(runs(from).length, 1);
    } finally {
      delete process.env.BYNK_FAKE_AGAINST;
    }
  });

  it("offers a retry that skips unsupported cases, after the refusal", async function () {
    this.timeout(60_000);
    process.env.BYNK_FAKE_AGAINST = "unsupported";
    let offeredButtons: string[] = [];
    (vscode.window as { showWarningMessage: unknown }).showWarningMessage = async (_m: string, ...items: string[]) => {
      offeredButtons = items.filter((i) => typeof i === "string");
      return offeredButtons[0];
    };
    try {
      const from = fake.read().length;
      choose("preview");
      await vscode.commands.executeCommand("bynk.runTestsAgainst");
      await waitFor(() => runs(from).length >= 2, "the refused run and its retry", 30_000);

      const [first, retry] = runs(from);
      assert.ok(!first.includes("--skip-unsupported"), "the first run does not skip");
      assert.deepStrictEqual(offeredButtons, ["Retry, skipping unsupported cases"]);
      assert.ok(retry.includes("--skip-unsupported"), "the retry skips unsupported cases");
      assert.deepStrictEqual(
        retry.slice(retry.indexOf("--against"), retry.indexOf("--against") + 2),
        ["--against", "preview"],
      );
    } finally {
      delete process.env.BYNK_FAKE_AGAINST;
    }
  });
});

describe("a run against a target reports only the system cases", () => {
  const ROOT = vscode.Uri.file(path.join(os.tmpdir(), "bynk-against-root"));
  const S = "bynk-test:unit:text";
  const TREE: PlanCase[] = [
    { suiteId: S, name: "a unit case", tier: "unit" },
    { suiteId: S, name: "a wired case", tier: "system" },
    { suiteId: S, name: "a skipped wired case", tier: "system" },
    { suiteId: S, name: "an integration case", tier: "integration" },
  ];
  let ctrl: vscode.TestController;
  before(() => {
    ctrl = vscode.tests.createTestController("bynk-against-test", "Against test");
  });
  after(() => ctrl.dispose());

  it("leaves the cases of other tiers as they were, and shows a skipped case as skipped", () => {
    const items = new Map<string, vscode.TestItem>();
    const suite = ctrl.createTestItem(S, "text");
    for (const c of TREE) {
      const item = ctrl.createTestItem(caseKey(S, c.name), c.name);
      suite.children.add(item);
      items.set(item.id, item);
    }
    const calls: string[] = [];
    const output: string[] = [];
    const sink: RunSink = {
      started: (t) => void calls.push(`started ${t.label}`),
      passed: (t) => void calls.push(`passed ${t.label}`),
      failed: (t) => void calls.push(`failed ${t.label}`),
      skipped: (t) => void calls.push(`skipped ${t.label}`),
      appendOutput: (o) => void output.push(o),
    };
    // What a run against a target returns when the document also lists a case
    // of another tier: it must not be reported.
    const doc: TestRun = {
      passed: 1,
      failed: 0,
      skipped: 1,
      transport: "remote:preview",
      suites: [
        {
          name: "text",
          kind: "unit",
          cases: [
            { name: "a wired case", outcome: "pass", tier: "system" },
            { name: "a skipped wired case", outcome: "skipped", message: "uses `advance Clock`", tier: "system" },
            { name: "a unit case", outcome: "pass", tier: "unit" },
          ],
        },
      ],
    };
    const system = systemOnly(TREE, {});
    const planned = planRun(system.cases, system.request, { remote: true });
    const reported = applyDocument(sink, items, { ...planned, narrowed: true }, doc, ROOT);

    assert.deepStrictEqual(calls, [
      "started a wired case",
      "passed a wired case",
      "started a skipped wired case",
      "skipped a skipped wired case",
    ]);
    assert.deepStrictEqual([...reported].sort(), [caseKey(S, "a skipped wired case"), caseKey(S, "a wired case")].sort());
    assert.ok(output.some((o) => o.includes("skipped: a skipped wired case — uses `advance Clock`")));
  });
});
