import * as assert from "assert";

import * as vscode from "vscode";

const EXT_ID = "bynk-lang.bynk-vscode";

function fixtureUri(rel: string): vscode.Uri {
  const folder = vscode.workspace.workspaceFolders?.[0];
  assert.ok(folder, "a fixture workspace folder is open");
  return vscode.Uri.joinPath(folder.uri, rel);
}

/** Poll `fn` until it yields a non-empty result or the timeout elapses. The
 *  language server analyses asynchronously, so LSP requests right after open
 *  can return nothing until the first analysis round lands. */
async function waitFor<T>(
  fn: () => Thenable<T | undefined>,
  what: string,
  timeoutMs = 40_000,
): Promise<T> {
  const start = Date.now();
  for (;;) {
    const value = await fn();
    const nonEmpty = Array.isArray(value) ? value.length > 0 : value !== undefined;
    if (nonEmpty) return value as T;
    if (Date.now() - start > timeoutMs) {
      throw new Error(`timed out after ${timeoutMs}ms waiting for ${what}`);
    }
    await new Promise((r) => setTimeout(r, 250));
  }
}

describe("Bynk extension", () => {
  let doc: vscode.TextDocument;

  before(async function () {
    this.timeout(120_000);
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    await ext.activate();
    doc = await vscode.workspace.openTextDocument(fixtureUri("src/text.bynk"));
    await vscode.window.showTextDocument(doc);
  });

  after(async () => {
    await vscode.commands.executeCommand("workbench.action.closeAllEditors");
  });

  it("activates and recognises the bynk language", () => {
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext?.isActive, "extension is active");
    assert.strictEqual(doc.languageId, "bynk", "the fixture file is a bynk doc");
  });

  it("registers the Test Explorer run command", async () => {
    // Proves `registerTesting` ran during activation (it creates the test
    // controller + run profile and registers this command). An actual run needs
    // `bynkc` on PATH, which the integration harness doesn't provide, so the
    // command registration is the activation-time smoke signal.
    const commands = await vscode.commands.getCommands(true);
    assert.ok(
      commands.includes("bynk.runTests"),
      "`bynk.runTests` is registered",
    );
  });

  it("serves go-to-definition from the language server", async () => {
    // The `shout` call inside the interpolation hole resolves to its fn def —
    // proving the client reached `running` against the local server.
    const callOffset = doc.getText().indexOf("shout(name)");
    const pos = doc.positionAt(callOffset + 1);
    const defs = await waitFor(
      () =>
        vscode.commands.executeCommand<vscode.Location[]>(
          "vscode.executeDefinitionProvider",
          doc.uri,
          pos,
        ),
      "definition of `shout`",
    );
    assert.ok(defs.length > 0, "at least one definition location");
  });

  it("serves references", async () => {
    const defOffset = doc.getText().indexOf("fn shout") + "fn ".length;
    const pos = doc.positionAt(defOffset + 1);
    const refs = await waitFor(
      () =>
        vscode.commands.executeCommand<vscode.Location[]>(
          "vscode.executeReferenceProvider",
          doc.uri,
          pos,
        ),
      "references of `shout`",
    );
    assert.ok(refs.length >= 1, "`shout` has at least one reference");
  });

  // The #143 / #144 regression. The `N references` CodeLens carries the
  // built-in `editor.action.showReferences` command, whose arguments the
  // server sends as plain LSP JSON. VS Code validates them with `instanceof`,
  // so without the client middleware (#144) the arguments are plain objects and
  // executing the lens throws "argument does not match one of these
  // constraints". The middleware re-hydrates them into real `Uri` / `Position`
  // / `Location[]`; this test pins that they arrive hydrated and that executing
  // the command does not throw.
  it("resolves and runs the reference CodeLens without an argument-constraint error", async () => {
    const lenses = await waitFor(
      () =>
        vscode.commands.executeCommand<vscode.CodeLens[]>(
          "vscode.executeCodeLensProvider",
          doc.uri,
        ),
      "code lenses",
    );
    const refLens = lenses.find(
      (l) => l.command?.command === "editor.action.showReferences",
    );
    assert.ok(refLens, "a `showReferences` CodeLens is present over a definition");

    const args = refLens.command?.arguments ?? [];
    assert.ok(args[0] instanceof vscode.Uri, "arg 0 is a hydrated Uri");
    assert.ok(args[1] instanceof vscode.Position, "arg 1 is a hydrated Position");
    assert.ok(
      Array.isArray(args[2]) && (args[2].length === 0 || args[2][0] instanceof vscode.Location),
      "arg 2 is a hydrated Location[]",
    );

    // Executing must not throw — the user-click path the bug lived on.
    await vscode.commands.executeCommand(
      refLens.command!.command,
      ...args,
    );
  });
});
