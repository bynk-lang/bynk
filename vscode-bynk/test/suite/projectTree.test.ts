// #2097: the Bynk project sidebar end to end, against the real language
// server: units grouped by kind, `consumes` edges in both directions,
// declarations from document symbols, handler actions opening the views, and
// a refresh after a saved edit. Read through the `bynk._projectTreeSnapshot`
// test seam `projectTree.ts` registers.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import * as vscode from "vscode";

const EXT_ID = "bynk.bynk-vscode";

interface Entry {
  label: string;
  contextValue?: string;
  description?: string;
  children?: Entry[];
}

const FILES: Record<string, string> = {
  "src/billing.bynk": `context billing

uses money

service Pricing {
  on call(sku: String, qty: Int) -> Effect[Int] {
    cents(qty)
  }
}
`,
  "src/storefront.bynk": `context storefront

consumes billing

service checkout {
  on call(sku: String, qty: Int) -> Effect[Int] {
    let q <- billing.Pricing(sku, qty)
    q
  }
}
`,
  "src/money.bynk": `commons money

fn cents(n: Int) -> Int {
  n * 100
}
`,
};

function snapshot(depth: number, reload = false): Thenable<Entry[]> {
  return vscode.commands.executeCommand<Entry[]>("bynk._projectTreeSnapshot", depth, reload);
}

function find(entries: Entry[] | undefined, ...labels: string[]): Entry | undefined {
  let list = entries;
  let hit: Entry | undefined;
  for (const l of labels) {
    hit = list?.find((e) => e.label === l);
    list = hit?.children;
  }
  return hit;
}

async function waitFor<T>(fn: () => Thenable<T> | T, ok: (v: T) => boolean, what: string, timeoutMs = 45_000): Promise<T> {
  const start = Date.now();
  for (;;) {
    const v = await fn();
    if (ok(v)) return v;
    if (Date.now() - start > timeoutMs) throw new Error(`timed out waiting for ${what}: ${JSON.stringify(v)}`);
    await new Promise((r) => setTimeout(r, 500));
  }
}

describe("Bynk project sidebar (#2097)", () => {
  let dir: string;
  let project: string;

  before(async function () {
    this.timeout(120_000);
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    await ext.activate();
    const workspace = path.resolve(__dirname, "../../../test/fixtures/workspace");
    dir = fs.realpathSync(fs.mkdtempSync(path.join(workspace, ".pt-")));
    project = path.basename(dir);
    fs.mkdirSync(path.join(dir, "src"));
    fs.writeFileSync(path.join(dir, "bynk.toml"), '[project]\nname = "pt"\n\n[paths]\ninclude = ["src"]\n');
    for (const [rel, text] of Object.entries(FILES)) fs.writeFileSync(path.join(dir, rel), text);
    // Open a file so the server analyses the project.
    await vscode.window.showTextDocument(
      await vscode.workspace.openTextDocument(vscode.Uri.file(path.join(dir, "src", "storefront.bynk"))),
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

  it("lists the project's units by kind, with consumes edges both ways", async function () {
    this.timeout(60_000);
    // The fixture workspace holds several projects, so each is a top node.
    const tree = await waitFor(
      () => snapshot(5, true),
      (t) => (find(t, project, "Contexts", "storefront", "Consumes")?.children?.length ?? 0) > 0,
      "the storefront unit's consumes edge",
    );
    const p = find(tree, project);
    assert.deepStrictEqual(
      p?.children?.map((g) => g.label),
      ["Contexts", "Commons"],
      "contexts and commons groups",
    );
    assert.deepStrictEqual(find(tree, project, "Contexts")?.children?.map((u) => u.label), ["billing", "storefront"]);
    assert.deepStrictEqual(find(tree, project, "Commons")?.children?.map((u) => u.label), ["money"]);
    assert.strictEqual(find(tree, project, "Contexts", "storefront")?.contextValue, "bynkUnit");
    assert.deepStrictEqual(
      find(tree, project, "Contexts", "storefront", "Consumes")?.children?.map((e) => e.label),
      ["billing"],
    );
    assert.deepStrictEqual(
      find(tree, project, "Contexts", "billing", "Consumed by")?.children?.map((e) => e.label),
      ["storefront"],
    );
  });

  it("expands a unit to its declarations, handlers marked for the view actions", async function () {
    this.timeout(30_000);
    const tree = await snapshot(6);
    const service = find(tree, project, "Contexts", "billing", "Pricing");
    assert.ok(service, `billing declares Pricing: ${JSON.stringify(find(tree, project, "Contexts", "billing"))}`);
    const handlers = service.children ?? [];
    assert.strictEqual(handlers.length, 1);
    assert.strictEqual(handlers[0].contextValue, "bynkHandler");
    assert.strictEqual(handlers[0].label, "on call(sku: String, qty: Int)", "labelled by its header line");
    const fn = find(tree, project, "Commons", "money", "cents");
    assert.strictEqual(fn?.contextValue, "bynkDeclaration");
  });

  it("opens the wire-contract view from a handler node", async function () {
    this.timeout(30_000);
    const uri = vscode.Uri.file(path.join(dir, "src", "billing.bynk")).toString();
    const line = FILES["src/billing.bynk"].split("\n").findIndex((l) => l.includes("on call("));
    await vscode.commands.executeCommand("bynk.project.wireContract", {
      type: "decl",
      uri,
      handler: true,
      label: "on call",
      symbol: { selectionRange: { start: { line, character: 2 } } },
    });
    const state = await waitFor(
      () => vscode.commands.executeCommand<{ open: boolean; target?: { uri: string } }>("bynk._liveViewState", "bynkWireContract"),
      (s) => s?.open === true,
      "the wire-contract view",
    );
    assert.strictEqual(state.target?.uri, uri);
  });

  it("reloads on the live views' trigger while visible, and shows the new declaration", async function () {
    this.timeout(60_000);
    const treeState = () =>
      vscode.commands.executeCommand<{ reloads: number; visible: boolean }>("bynk._projectTreeState");
    // Show the Bynk activity-bar container so the tree is visible.
    await vscode.commands.executeCommand("workbench.view.extension.bynk");
    await waitFor(treeState, (s) => s.visible, "the project view to be visible", 10_000);
    const before = await treeState();

    const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(path.join(dir, "src", "money.bynk")));
    const editor = await vscode.window.showTextDocument(doc);
    await editor.edit((e) => e.insert(new vscode.Position(doc.lineCount, 0), "\nfn dollars(n: Int) -> Int {\n  n\n}\n"));
    await doc.save();
    await waitFor(treeState, (s) => s.reloads > before.reloads, "a reload after the save");
    await waitFor(
      () => snapshot(4),
      (t) => find(t, project, "Commons", "money", "dollars") !== undefined,
      "the new declaration",
    );
  });
});
