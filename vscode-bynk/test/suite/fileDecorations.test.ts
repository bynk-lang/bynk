// #2478: the Explorer file decorations, inside the extension host: a context
// badge per file (off for a single-context project), the failing-test tint
// set by a failed case and cleared by a passing re-run, and the settings.
// Read through the `bynk._fileDecoration` test seam `fileDecorations.ts` registers.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { applyDocument, type RunSink } from "../../src/testing";
import { caseKey, planRun, type TestRun } from "../../src/testPlan";

const EXT_ID = "bynk-lang.bynk-vscode";
const FIXTURES = path.resolve(__dirname, "../../../test/fixtures");

interface Decoration {
  badge?: string;
  tooltip?: string;
  failed: boolean;
}

const decoration = (file: string): Thenable<Decoration> =>
  vscode.commands.executeCommand<Decoration>("bynk._fileDecoration", vscode.Uri.file(file).toString());

async function waitFor<T>(fn: () => Thenable<T>, ok: (v: T) => boolean, what: string, timeoutMs = 15_000): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const v = await fn();
    if (ok(v)) return v;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}: ${JSON.stringify(v)}`);
    await new Promise((r) => setTimeout(r, 100));
  }
}

const sink: RunSink = { started() {}, passed() {}, failed() {}, skipped() {}, appendOutput() {} };

describe("Bynk Explorer file decorations (#2478)", () => {
  let dir: string;
  let billing: string;
  let ctrl: vscode.TestController;

  before(async function () {
    this.timeout(60_000);
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    await ext.activate();
    const workspace = path.resolve(__dirname, "../../../test/fixtures/workspace");
    dir = fs.realpathSync(fs.mkdtempSync(path.join(workspace, ".fd-")));
    fs.mkdirSync(path.join(dir, "src"));
    fs.writeFileSync(path.join(dir, "bynk.toml"), '[project]\nname = "fd"\n\n[paths]\ninclude = ["src"]\n');
    billing = path.join(dir, "src", "billing.bynk");
    fs.writeFileSync(billing, "context billing\n");
    fs.writeFileSync(path.join(dir, "src", "money.bynk"), "commons money\n");
    ctrl = vscode.tests.createTestController("bynk-filedecorations-test", "File decorations test");
  });

  after(async () => {
    ctrl?.dispose();
    await vscode.workspace.getConfiguration("bynk.fileDecorations").update("contextBadge", undefined, true);
    if (dir) fs.rmSync(dir, { recursive: true, force: true });
  });

  it("shows no badge in a single-context project, until asked for", async () => {
    await waitFor(() => decoration(billing), (d) => d.badge === undefined, "scan to settle");
    assert.strictEqual((await decoration(path.join(dir, "src", "money.bynk"))).badge, undefined);
    await vscode.workspace.getConfiguration("bynk.fileDecorations").update("contextBadge", "always", true);
    const d = await waitFor(() => decoration(billing), (x) => x.badge === "BI", "the always badge");
    assert.strictEqual(d.tooltip, "Bynk context billing");
    // Commons carry none.
    assert.strictEqual((await decoration(path.join(dir, "src", "money.bynk"))).badge, undefined);
  });

  it("badges every context once the project has a second", async () => {
    await vscode.workspace.getConfiguration("bynk.fileDecorations").update("contextBadge", "auto", true);
    fs.writeFileSync(path.join(dir, "src", "storefront.bynk"), "context storefront\n");
    await waitFor(() => decoration(billing), (d) => d.badge === "BI", "the multi-context badge");
    assert.strictEqual((await decoration(path.join(dir, "src", "storefront.bynk"))).badge, "ST");
    await vscode.workspace.getConfiguration("bynk.fileDecorations").update("contextBadge", "never", true);
    await waitFor(() => decoration(billing), (d) => d.badge === undefined, "the badge to switch off");
  });

  it("tints a file whose test failed and clears it when a re-run passes", async () => {
    const doc = JSON.parse(fs.readFileSync(path.join(FIXTURES, "test-run-remote.json"), "utf8")) as TestRun;
    const suite = doc.suites![0];
    const only = suite.cases.find((c) => c.outcome === "pass")!;
    const suiteItem = ctrl.createTestItem("suite", "suite");
    ctrl.items.add(suiteItem);
    const id = caseKey(`bynk-test:${suite.kind}:${suite.name}`, only.name);
    const item = ctrl.createTestItem(id, only.name, vscode.Uri.file(billing));
    suiteItem.children.add(item);
    const items = new Map([[id, item]]);
    const report = (outcome: string) => {
      const run: TestRun = { ...doc, suites: [{ ...suite, cases: [{ ...only, outcome }] }] };
      const plan = planRun([{ suiteId: id.slice(0, id.indexOf("::")), name: only.name }], {});
      applyDocument(sink, items, { ...plan, narrowed: false }, run, vscode.Uri.file(dir));
    };

    report("fail");
    assert.ok((await decoration(billing)).failed, "a failed case tints its file");
    await vscode.workspace.getConfiguration("bynk.fileDecorations").update("failedTests", false, true);
    assert.ok(!(await decoration(billing)).failed, "the setting turns the tint off");
    await vscode.workspace.getConfiguration("bynk.fileDecorations").update("failedTests", undefined, true);
    report("pass");
    assert.ok(!(await decoration(billing)).failed, "a passing re-run clears the tint");
  });
});
