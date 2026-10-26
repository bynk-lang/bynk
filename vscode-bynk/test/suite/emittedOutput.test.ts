// #2471: "Bynk: Show Compiled Output" end to end against the real CLI: the
// command opens the file's generated TypeScript (a read-only `bynk-emitted:`
// document) beside the source, compiled from the unsaved buffer, and the
// document follows edits while it is open.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import * as vscode from "vscode";

const EXT_ID = "bynk-lang.bynk-vscode";

const SRC = (greeting: string) => `context probe.emitted

service api from http {
  on GET("/hello") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("${greeting}")
  }
}
`;

async function waitFor<T>(fn: () => T | undefined, what: string, timeoutMs = 40_000): Promise<T> {
  const start = Date.now();
  for (;;) {
    const v = fn();
    if (v !== undefined) return v;
    if (Date.now() - start > timeoutMs) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 250));
  }
}

const emittedDoc = () =>
  vscode.workspace.textDocuments.find((d) => d.uri.scheme === "bynk-emitted");

describe("Bynk compiled output (#2471)", () => {
  const workspace = path.resolve(__dirname, "../../../test/fixtures/workspace");
  // The shared fixture project doesn't compile (its units sit outside the
  // `fixture/` layout), and a build needs every file clean, so the probe gets
  // a project of its own.
  const project = path.join(workspace, "emitted_probe");
  const probe = path.join(project, "src", "probe", "emitted.bynk");
  let doc: vscode.TextDocument;

  before(async function () {
    this.timeout(120_000);
    fs.mkdirSync(path.dirname(probe), { recursive: true });
    fs.writeFileSync(path.join(project, "bynk.toml"), '[project]\nname = "probe"\n');
    fs.writeFileSync(probe, SRC("hello"));
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    await ext.activate();
    doc = await vscode.workspace.openTextDocument(vscode.Uri.file(probe));
    await vscode.window.showTextDocument(doc);
  });

  after(async () => {
    await vscode.commands.executeCommand("workbench.action.closeAllEditors");
    fs.rmSync(project, { recursive: true, force: true });
  });

  it("opens the generated TypeScript beside the source, and follows unsaved edits", async function () {
    this.timeout(120_000);
    // The server's first round may not have committed yet; retry until it has.
    let uri: string | undefined;
    const start = Date.now();
    while (!uri && Date.now() - start < 60_000) {
      uri = await vscode.commands.executeCommand<string | undefined>(
        "bynk.showCompiledOutput",
        doc.uri,
      );
      if (!uri) await new Promise((r) => setTimeout(r, 500));
    }
    assert.ok(uri, "the command opened a compiled-output document");
    const out = await waitFor(() => emittedDoc(), "the bynk-emitted document");
    assert.ok(out.uri.path.endsWith(".ts"), `a TypeScript module: ${out.uri.path}`);
    assert.ok(out.getText().includes("hello"), "it holds the generated handler");

    // An edit that is not saved: the open view follows the buffer, not the disk.
    const edit = new vscode.WorkspaceEdit();
    edit.replace(doc.uri, new vscode.Range(0, 0, doc.lineCount, 0), SRC("goodbye"));
    await vscode.workspace.applyEdit(edit);
    assert.ok(doc.isDirty, "the edit is unsaved");
    await waitFor(
      () => (emittedDoc()?.getText().includes("goodbye") ? true : undefined),
      "the view to follow the unsaved edit",
    );
    assert.ok(!fs.readFileSync(probe, "utf8").includes("goodbye"), "disk is unchanged");
  });
});
