// #2093: the live model views end to end, against the real language server —
// refresh on save, follow the cursor (and the pin), and restore after a
// reload (driven through the serializer's restore path, since a test cannot
// reload its own window). State is read through the `bynk._liveView*` test
// seams `extension.ts` registers.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import * as vscode from "vscode";

const EXT_ID = "bynk.bynk-vscode";
const SEQ = "bynkSequenceDiagram";
const DOC = "bynkDocumentation";

interface Snapshot {
  open: boolean;
  visible: boolean;
  target: { uri: string; position?: { line: number; character: number }; pinned?: boolean } | undefined;
  handler: string | undefined;
  refreshes: number;
  updates: number;
  hasPayload: boolean;
}

const SRC = `context shop

service api from http {
  on GET("/a") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("a")
  }

  on GET("/b") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("b")
  }
}
`;

function state(viewType: string): Thenable<Snapshot> {
  return vscode.commands.executeCommand<Snapshot>("bynk._liveViewState", viewType);
}

async function waitFor<T>(fn: () => Thenable<T> | T, ok: (v: T) => boolean, what: string, timeoutMs = 30_000): Promise<T> {
  const start = Date.now();
  for (;;) {
    const v = await fn();
    if (ok(v)) return v;
    if (Date.now() - start > timeoutMs) throw new Error(`timed out waiting for ${what}: ${JSON.stringify(v)}`);
    await new Promise((r) => setTimeout(r, 200));
  }
}

function lineOf(needle: string): number {
  return SRC.split("\n").findIndex((l) => l.includes(needle));
}

describe("live model views (#2093)", () => {
  let dir: string;
  let uri: vscode.Uri;
  let editor: vscode.TextEditor;

  before(async function () {
    this.timeout(120_000);
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    await ext.activate();

    const workspace = path.resolve(__dirname, "../../../test/fixtures/workspace");
    dir = fs.realpathSync(fs.mkdtempSync(path.join(workspace, ".lv-")));
    fs.mkdirSync(path.join(dir, "src"));
    fs.writeFileSync(path.join(dir, "bynk.toml"), '[project]\nname = "lv"\n\n[paths]\ninclude = ["src"]\n');
    fs.writeFileSync(path.join(dir, "src", "shop.bynk"), SRC);
    uri = vscode.Uri.file(path.join(dir, "src", "shop.bynk"));
    editor = await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(uri));

    // Wait for the server to have committed a round for the new project: the
    // sequence request answers once it has.
    const a = new vscode.Position(lineOf('GET("/a")') + 1, 4);
    await waitFor(
      async () => {
        await vscode.commands.executeCommand("bynk.showSequenceDiagram", uri.toString(), a);
        return state(SEQ);
      },
      (s) => s?.open === true && s.hasPayload,
      "the sequence view to open",
      60_000,
    );
  });

  after(async () => {
    await vscode.commands.executeCommand("workbench.action.closeAllEditors");
    if (dir) {
      try {
        fs.rmSync(dir, { recursive: true, force: true });
      } catch {
        /* best-effort */
      }
    }
  });

  it("targets the handler the command was run on", async () => {
    const s = await state(SEQ);
    assert.strictEqual(s.target?.uri, uri.toString());
    assert.strictEqual(s.target?.position?.line, lineOf('GET("/a")') + 1);
    assert.ok(s.handler, "the enclosing handler is remembered");
  });

  it("follows the cursor into another handler", async function () {
    this.timeout(30_000);
    const before = await state(SEQ);
    editor = await vscode.window.showTextDocument(editor.document, vscode.ViewColumn.One);
    const b = new vscode.Position(lineOf('GET("/b")') + 1, 4);
    editor.selection = new vscode.Selection(b, b);
    const after = await waitFor(() => state(SEQ), (s) => s.handler !== before.handler, "a retarget");
    assert.strictEqual(after.target?.position?.line, b.line);
  });

  it("stays put inside one handler and between handlers", async function () {
    this.timeout(10_000);
    const before = await state(SEQ);
    const inside = new vscode.Position(lineOf('GET("/b")'), 2);
    editor.selection = new vscode.Selection(inside, inside);
    const between = new vscode.Position(lineOf('GET("/b")') - 1, 0);
    editor.selection = new vscode.Selection(between, between);
    await new Promise((r) => setTimeout(r, 800));
    assert.strictEqual((await state(SEQ)).handler, before.handler);
  });

  it("does not retarget while pinned, and catches up when unpinned", async function () {
    this.timeout(30_000);
    await vscode.commands.executeCommand("bynk._liveViewPin", SEQ, true);
    const pinned = await state(SEQ);
    assert.strictEqual(pinned.target?.pinned, true);
    const a = new vscode.Position(lineOf('GET("/a")') + 1, 4);
    editor.selection = new vscode.Selection(a, a);
    await new Promise((r) => setTimeout(r, 800));
    assert.strictEqual((await state(SEQ)).handler, pinned.handler, "pinned view holds");

    await vscode.commands.executeCommand("bynk._liveViewPin", SEQ, false);
    const after = await waitFor(() => state(SEQ), (s) => s.handler !== pinned.handler, "the unpinned retarget");
    assert.strictEqual(after.target?.position?.line, a.line);
  });

  it("refreshes after an edit is saved (onSave)", async function () {
    this.timeout(30_000);
    // The panel must be visible to refresh; reveal it beside the editor.
    await vscode.commands.executeCommand("bynk.showSequenceDiagram", uri.toString(), editor.selection.active);
    const before = await state(SEQ);
    const ok = await editor.edit((e) => e.insert(new vscode.Position(0, 0), "-- edited\n"));
    assert.ok(ok, "edit applied");
    await editor.document.save();
    // The inserted line shifts every range in the model, so the panel only
    // updates once a refresh sees the post-save round — the catch-up the
    // arm-until-the-round-commits scheduling exists for (a re-pull at the
    // moment of the save can still read the previous round).
    const after = await waitFor(
      () => state(SEQ),
      (s) => s.refreshes > before.refreshes && s.updates > before.updates,
      "the panel to show the post-save model",
    );
    // The target moved down with the inserted line.
    assert.strictEqual(after.target?.position?.line, (before.target?.position?.line ?? 0) + 1);
  });

  it("offers the Wire contract lens only when bynk.viewsCodeLens is on", async function () {
    this.timeout(30_000);
    const lenses = async () =>
      ((await vscode.commands.executeCommand<vscode.CodeLens[]>("vscode.executeCodeLensProvider", uri)) ?? [])
        .map((l) => l.command?.command)
        .filter((c) => c === "bynk.showSequenceDiagram" || c === "bynk.showWireContract");
    // The server's own `Show Sequence` lens is always there, one per handler.
    const off = await waitFor(lenses, (l) => l.length >= 2, "the server's sequence lenses");
    assert.ok(!off.includes("bynk.showWireContract"), "off by default (DECISION D)");
    const cfg = vscode.workspace.getConfiguration("bynk");
    await cfg.update("viewsCodeLens", true, vscode.ConfigurationTarget.Global);
    try {
      const on = await waitFor(lenses, (l) => l.includes("bynk.showWireContract"), "the wire-contract lenses");
      assert.strictEqual(on.filter((c) => c === "bynk.showWireContract").length, 2, "one per handler");
      assert.strictEqual(on.filter((c) => c === "bynk.showSequenceDiagram").length, 2, "no second sequence lens");
    } finally {
      await cfg.update("viewsCodeLens", undefined, vscode.ConfigurationTarget.Global);
    }
  });

  // #2108 review: an `editor/title` button calls the command with
  // `(resourceUri, {groupId, editorIndex})`, which is not a position.
  it("takes the cursor of the shown editor when a title-bar button passes no position", async function () {
    this.timeout(30_000);
    editor = await vscode.window.showTextDocument(editor.document, vscode.ViewColumn.One);
    const b = new vscode.Position(lineOf('GET("/b")') + 1, 4);
    editor.selection = new vscode.Selection(b, b);
    await vscode.commands.executeCommand("bynk.showWireContract", uri, { groupId: 1, editorIndex: 0 });
    const s = await waitFor(
      () => vscode.commands.executeCommand<Snapshot>("bynk._liveViewState", "bynkWireContract"),
      (v) => v?.open === true,
      "the wire-contract view",
    );
    assert.deepStrictEqual(s.target?.position, { line: b.line, character: b.character });
  });

  // #2108 review: a page can draw on other files (a multi-file unit's
  // documentation, a consumed context), so an edit to another file refreshes.
  it("refreshes a documentation page when another file is saved", async function () {
    this.timeout(60_000);
    await vscode.commands.executeCommand("bynk.showDocumentation", uri.toString());
    const before = await waitFor(() => state(DOC), (v) => v.open && v.hasPayload, "the documentation view");
    const other = vscode.Uri.file(path.join(dir, "src", "notes.bynk"));
    fs.writeFileSync(other.fsPath, "commons notes\n\nfn n() -> Int {\n  1\n}\n");
    const doc = await vscode.workspace.openTextDocument(other);
    const ed = await vscode.window.showTextDocument(doc, vscode.ViewColumn.One);
    await ed.edit((e) => e.insert(new vscode.Position(doc.lineCount, 0), "\nfn m() -> Int {\n  2\n}\n"));
    await doc.save();
    await waitFor(() => state(DOC), (v) => v.refreshes > before.refreshes, "a documentation refresh");
  });

  it("restores a panel from its persisted state", async function () {
    this.timeout(60_000);
    const panel = vscode.window.createWebviewPanel(DOC, "restored", vscode.ViewColumn.Two, {});
    await vscode.commands.executeCommand("bynk._liveViewRestore", DOC, panel, { uri: uri.toString() });
    const s = await waitFor(() => state(DOC), (v) => v.hasPayload, "the restored documentation view");
    assert.strictEqual(s.target?.uri, uri.toString());
    panel.dispose();
  });

  it("restores a panel whose file is gone to the empty state, not an error", async function () {
    this.timeout(60_000);
    const panel = vscode.window.createWebviewPanel(DOC, "restored", vscode.ViewColumn.Two, {});
    const gone = vscode.Uri.file(path.join(dir, "src", "gone.bynk")).toString();
    await vscode.commands.executeCommand("bynk._liveViewRestore", DOC, panel, { uri: gone });
    const s = await state(DOC);
    assert.ok(s.open, "the panel is adopted");
    assert.ok(!s.hasPayload, "with nothing to show");
    panel.dispose();
  });
});
