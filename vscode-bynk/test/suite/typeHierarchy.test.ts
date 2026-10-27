import * as assert from "assert";

import * as vscode from "vscode";

const EXT_ID = "bynk-lang.bynk-vscode";

/** Poll `fn` until it yields a non-empty result or the timeout elapses. The
 *  server registers type hierarchy dynamically at `initialized` and analyses
 *  asynchronously, so a request right after open can come back empty. */
async function waitFor<T>(
  fn: () => Thenable<T[] | undefined>,
  what: string,
  timeoutMs = 40_000,
): Promise<T[]> {
  const start = Date.now();
  for (;;) {
    const value = await fn();
    if (value && value.length > 0) return value;
    if (Date.now() - start > timeoutMs) {
      throw new Error(`timed out after ${timeoutMs}ms waiting for ${what}`);
    }
    await new Promise((r) => setTimeout(r, 250));
  }
}

describe("Bynk type hierarchy", () => {
  let doc: vscode.TextDocument;

  before(async function () {
    this.timeout(120_000);
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    await ext.activate();
    const folder = vscode.workspace.workspaceFolders?.[0];
    assert.ok(folder, "a fixture workspace folder is open");
    doc = await vscode.workspace.openTextDocument(
      vscode.Uri.joinPath(folder.uri, "src/quantity.bynk"),
    );
    await vscode.window.showTextDocument(doc);
  });

  after(async () => {
    await vscode.commands.executeCommand("workbench.action.closeAllEditors");
  });

  it("walks a refined type to its primitive and back", async function () {
    this.timeout(60_000);
    // The server's type hierarchy is a dynamic registration, so this proves the
    // client picked it up rather than just that the handlers exist.
    const pos = doc.positionAt(doc.getText().indexOf("Qty") + 1);
    const items = await waitFor(
      () =>
        vscode.commands.executeCommand<vscode.TypeHierarchyItem[]>(
          "vscode.prepareTypeHierarchy",
          doc.uri,
          pos,
        ),
      "type hierarchy prepare on `Qty`",
    );
    assert.strictEqual(items[0].name, "Qty");

    const supers = await vscode.commands.executeCommand<
      vscode.TypeHierarchyItem[]
    >("vscode.provideSupertypes", items[0]);
    assert.deepStrictEqual(
      supers.map((i) => i.name),
      ["Int"],
    );
    // The primitive is anchored at `Int` in the declaration, not elsewhere.
    const intOffset = doc.getText().indexOf("Int");
    assert.strictEqual(
      supers[0].range.start.line,
      doc.positionAt(intOffset).line,
    );
    assert.strictEqual(
      supers[0].range.start.character,
      doc.positionAt(intOffset).character,
    );

    const subs = await vscode.commands.executeCommand<
      vscode.TypeHierarchyItem[]
    >("vscode.provideSubtypes", supers[0]);
    assert.ok(
      subs.some((i) => i.name === "Qty"),
      `\`Qty\` is among the subtypes of \`Int\`: ${subs.map((i) => i.name)}`,
    );
  });

  it("offers the hierarchy on a primitive written as a base", async function () {
    this.timeout(60_000);
    const pos = doc.positionAt(doc.getText().indexOf("Int") + 1);
    const items = await waitFor(
      () =>
        vscode.commands.executeCommand<vscode.TypeHierarchyItem[]>(
          "vscode.prepareTypeHierarchy",
          doc.uri,
          pos,
        ),
      "type hierarchy prepare on `Int`",
    );
    assert.strictEqual(items[0].name, "Int");
    const subs = await vscode.commands.executeCommand<
      vscode.TypeHierarchyItem[]
    >("vscode.provideSubtypes", items[0]);
    assert.ok(subs.some((i) => i.name === "Qty"));
  });
});
