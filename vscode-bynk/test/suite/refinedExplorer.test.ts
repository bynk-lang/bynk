// #2470: end-to-end coverage for the refined-type explorer. Hovering a refined
// type lists its predicates and carries a "Try values…" link to
// `bynk.exploreRefinedType` (only because this client advertises
// `experimental.refinedTypeExplorerCommand`), the middleware trusts that one
// command, and running it fetches `bynk/refinedTypeModel` and opens the panel.
// The command resolves to the model it showed, which is what is read here.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import * as vscode from "vscode";

const EXT_ID = "bynk-lang.bynk-vscode";

interface Model {
  name: string;
  base: string;
  clauses: Array<{ source: string; description: string; check: string }>;
  examples: string[];
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

describe("Bynk refined-type explorer (#2470)", () => {
  const workspace = path.resolve(__dirname, "../../../test/fixtures/workspace");
  const probe = path.join(workspace, "src", "refined_probe.bynk");
  const source =
    "commons fixture.refined_probe\n\ntype Quantity = Int where InRange(1, 100)\n\nfn f(q: Quantity) -> Int {\n  1\n}\n";
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

  it("hovers with the predicates and a trusted Try values… link, and the command shows the model", async function () {
    this.timeout(60_000);
    const at = doc.positionAt(source.indexOf("Quantity)") + 1);
    const hovers = await waitFor(
      () => vscode.commands.executeCommand<vscode.Hover[]>("vscode.executeHoverProvider", doc.uri, at),
      (list) => list.some((h) => h.contents.some((c) => typeof c !== "string" && "value" in c && c.value.includes("Try values"))),
      "the refined type's hover",
    );
    const md = hovers
      .flatMap((h) => h.contents)
      .find((c): c is vscode.MarkdownString => typeof c !== "string" && "value" in c && c.value.includes("Try values"))!;
    assert.ok(md.value.includes("`InRange(1, 100)` — 1 ≤ value ≤ 100"), md.value);
    assert.deepStrictEqual(
      (md.isTrusted as { enabledCommands: string[] }).enabledCommands,
      ["bynk.exploreRefinedType"],
    );

    const model = await vscode.commands.executeCommand<Model | undefined>(
      "bynk.exploreRefinedType",
      doc.uri.toString(),
      { line: at.line, character: at.character },
    );
    assert.ok(model, "the command shows a model");
    assert.strictEqual(model.name, "Quantity");
    assert.strictEqual(model.base, "Int");
    assert.strictEqual(model.clauses[0].source, "InRange(1, 100)");
    assert.deepStrictEqual(model.examples.slice(0, 2), ["1", "100"]);
  });
});
