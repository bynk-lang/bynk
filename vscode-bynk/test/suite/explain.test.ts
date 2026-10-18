// #2098: end-to-end coverage for the in-editor explanation. A file with an
// unknown type gets a `bynk.resolve.unknown_type` diagnostic; the server
// offers an "Explain …" quick fix for it (only because this client advertises
// `experimental.explainDiagnosticCommand`), and running the action's command
// fetches the explanation over `bynk/explain` and shows it. The command
// resolves to what it showed, which is what these tests read.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import * as vscode from "vscode";

const EXT_ID = "bynk.bynk-vscode";
const CODE = "bynk.resolve.unknown_type";

interface Explanation {
  title: string;
  markdown: string;
  href: string;
}

async function waitFor<T>(
  fn: () => Thenable<T | undefined>,
  ok: (value: T) => boolean,
  what: string,
  timeoutMs = 40_000,
): Promise<T> {
  const start = Date.now();
  for (;;) {
    const value = await fn();
    if (value !== undefined && ok(value)) return value;
    if (Date.now() - start > timeoutMs) {
      throw new Error(`timed out after ${timeoutMs}ms waiting for ${what}`);
    }
    await new Promise((r) => setTimeout(r, 250));
  }
}

describe("Bynk explain (#2098)", () => {
  const workspace = path.resolve(__dirname, "../../../test/fixtures/workspace");
  const probe = path.join(workspace, "src", "explain_probe.bynk");
  const source = "commons fixture.explain_probe\n\nfn f(u: Usr) -> Int {\n  1\n}\n";
  let doc: vscode.TextDocument;

  before(async function () {
    this.timeout(120_000);
    fs.writeFileSync(probe, source);
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    await ext.activate();
    doc = await vscode.workspace.openTextDocument(vscode.Uri.file(probe));
    await vscode.window.showTextDocument(doc);
  });

  after(async () => {
    await vscode.commands.executeCommand("workbench.action.closeAllEditors");
    fs.rmSync(probe, { force: true });
  });

  it("offers an Explain quick fix that runs the explain command with the code", async function () {
    this.timeout(60_000);
    const at = doc.positionAt(source.indexOf("Usr") + 1);
    const actions = await waitFor(
      () =>
        vscode.commands.executeCommand<vscode.CodeAction[]>(
          "vscode.executeCodeActionProvider",
          doc.uri,
          new vscode.Range(at, at),
        ),
      (list) => list.some((a) => a.title === `Explain \`${CODE}\``),
      "the Explain quick fix",
    );
    const action = actions.find((a) => a.title === `Explain \`${CODE}\``)!;
    assert.strictEqual(action.kind?.value, "quickfix");
    assert.strictEqual(action.isPreferred, false);
    assert.strictEqual(action.command?.command, "bynk.explainDiagnostic");
    assert.deepStrictEqual(action.command?.arguments, [CODE]);

    const shown = await vscode.commands.executeCommand<Explanation | null>(
      action.command!.command,
      ...(action.command!.arguments ?? []),
    );
    assert.ok(shown, "the curated code is explained");
    assert.strictEqual(shown.title, CODE);
    assert.ok(shown.markdown.includes("```bynk"), shown.markdown);
    assert.strictEqual(shown.href, "https://bynk-lang.org/book/reference/types/");
  });

  it("reports an uncurated code gracefully, without a panel", async () => {
    const shown = await vscode.commands.executeCommand<Explanation | null>(
      "bynk.explainDiagnostic",
      "bynk.resolve.duplicate_type",
    );
    assert.strictEqual(shown, null);
  });
});
