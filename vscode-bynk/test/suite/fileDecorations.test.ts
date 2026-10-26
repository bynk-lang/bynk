// #2478: the Explorer file decorations, inside the extension host: a context
// badge per file (off for a single-context project), the failing-test tint
// set by a failed case and cleared by a passing re-run, and the settings.
// Read through the `bynk._fileDecoration` test seam `fileDecorations.ts` registers.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { useFakeBynk } from "./fakeBynk";

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

describe("Bynk Explorer file decorations (#2478)", () => {
  let dir: string;
  let billing: string;

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
  });

  after(async () => {
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

  it("tints a file whose test failed and clears it when a re-run passes", async function () {
    // The stand-in `bynk` serves the run document; the extension host has its own
    // copy of the failure state, so the run has to go through the real Test Explorer path.
    this.timeout(120_000);
    const textFile = vscode.Uri.file(path.join(FIXTURES, "workspace", "src", "text.bynk"));
    const docDir = fs.mkdtempSync(path.join(os.tmpdir(), "bynk-decor-"));
    const fake = await useFakeBynk();
    try {
      const serve = (outcome: string) => {
        const doc = {
          passed: outcome === "pass" ? 1 : 0,
          failed: outcome === "pass" ? 0 : 1,
          suites: [
            {
              name: "text",
              kind: "unit",
              cases: [{ name: "tint probe", outcome, location: { path: "src/text.bynk", line: 1, col: 1 } }],
            },
          ],
        };
        const file = path.join(docDir, `doc-${outcome}.json`);
        fs.writeFileSync(file, JSON.stringify(doc));
        process.env.BYNK_FAKE_TEST_DOC = file;
      };

      serve("fail");
      await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(textFile));
      await vscode.commands.executeCommand("testing.refreshTests");
      await vscode.commands.executeCommand("bynk.runTestsInFile", textFile);
      await waitFor(() => decoration(textFile.fsPath), (d) => d.failed, "the failed run to tint the file", 45_000);

      await vscode.workspace.getConfiguration("bynk.fileDecorations").update("failedTests", false, true);
      await waitFor(() => decoration(textFile.fsPath), (d) => !d.failed, "the setting to turn the tint off");
      await vscode.workspace.getConfiguration("bynk.fileDecorations").update("failedTests", undefined, true);

      serve("pass");
      await vscode.commands.executeCommand("testing.refreshTests");
      await vscode.commands.executeCommand("bynk.runTestsInFile", textFile);
      await waitFor(() => decoration(textFile.fsPath), (d) => !d.failed, "a passing re-run to clear the tint", 45_000);
    } finally {
      delete process.env.BYNK_FAKE_TEST_DOC;
      await fake.restore();
      fs.rmSync(docDir, { recursive: true, force: true });
    }
  });
});
