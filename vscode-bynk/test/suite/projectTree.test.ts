// #2097: the Bynk project sidebar end to end, against the real language
// server: units grouped by kind, `consumes` edges in both directions,
// declarations from document symbols, handler actions opening the views, and
// a refresh after a saved edit. Read through the `bynk._projectTreeSnapshot`
// test seam `projectTree.ts` registers.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import * as vscode from "vscode";

const EXT_ID = "bynk-lang.bynk-vscode";

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
      ["Contexts", "Commons", "State Migrations"],
      "contexts and commons groups, then the migrations node (#2468)",
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

  // #2112 review: an editor save reloads the tree once (the hub's trigger),
  // not again from the file watcher's echo of the same save; under `manual`
  // it doesn't reload at all, whether the change came from the editor or
  // from outside it.
  it("respects bynk.views.refresh: bounded reloads per save, then quiet; none under manual", async function () {
    this.timeout(120_000);
    const treeState = () =>
      vscode.commands.executeCommand<{
        reloads: number;
        visible: boolean;
        hub: { mode: string; armed: boolean };
      }>("bynk._projectTreeState");
    // #2144: the configuration change reaches the extension asynchronously,
    // after `update()` resolves. Wait for the hub to be in the mode the test
    // sets, or a later step runs under the previous one.
    const modeIs = (mode: string) => waitFor(treeState, (st) => st.hub.mode === mode, `refresh mode ${mode}`, 10_000);
    // A save arms the scheduler: it fires at once, at the settle timer, and
    // on backed-off follow-ups for about 30 s (#2144), then disarms.
    const quiet = () => waitFor(treeState, (st) => !st.hub.armed, "the scheduler to disarm", 45_000);
    await vscode.commands.executeCommand("workbench.view.extension.bynk");
    await waitFor(treeState, (s) => s.visible, "the project view to be visible", 10_000);
    const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(path.join(dir, "src", "money.bynk")));
    const editor = await vscode.window.showTextDocument(doc);
    const pause = (ms: number) => new Promise((r) => setTimeout(r, ms));

    // onSave (the default): the save reloads the tree; the settle timer and
    // follow-ups may reload it a few more times (each one rescans), but the
    // watcher's echo of the save adds nothing, the count is bounded, and once
    // the scheduler disarms nothing reloads.
    await quiet();
    const before = (await treeState()).reloads;
    await editor.edit((e) => e.insert(new vscode.Position(doc.lineCount, 0), "\n// touched\n"));
    await doc.save();
    await waitFor(treeState, (st) => st.reloads > before, "the save to reload the tree", 10_000);
    await quiet();
    const afterSave = (await treeState()).reloads;
    // 1 at the save + 1 at settle + 4 follow-ups, plus a committed round or two.
    assert.ok(afterSave - before <= 9, `a bounded number of reloads per save, got ${afterSave - before}`);
    await pause(3000);
    assert.strictEqual((await treeState()).reloads, afterSave, "and nothing reloads once it is quiet");

    const cfg = vscode.workspace.getConfiguration("bynk");
    await cfg.update("views.refresh", "manual", vscode.ConfigurationTarget.Global);
    let failure: unknown;
    try {
      await modeIs("manual");
      const start = (await treeState()).reloads;
      await editor.edit((e) => e.insert(new vscode.Position(doc.lineCount, 0), "\n// again\n"));
      await doc.save();
      fs.writeFileSync(path.join(dir, "src", "extra.bynk"), "commons extra\n\nfn x() -> Int {\n  1\n}\n");
      await pause(3000);
      assert.strictEqual((await treeState()).reloads, start, "manual: no reload on a save or an external change");
    } catch (err) {
      failure = err;
      throw err;
    } finally {
      await cfg.update("views.refresh", undefined, vscode.ConfigurationTarget.Global);
      // Don't let a failure to see the reset mask the test's own failure.
      try {
        await modeIs("onSave");
      } catch (resetErr) {
        if (failure === undefined) throw resetErr;
      }
    }
  });

  // #2112 review: a change made outside the editor refreshes the
  // architecture model too, not just the header scan. #2144: this failed in
  // CI because the server's round for an external change can land after the
  // scheduler's settle timer, and a clean project publishes no diagnostics to
  // signal it. This project sets a debounce longer than the settle window so
  // that case is exercised on every run, not just on a slow machine.
  it("updates the consumes edges when a file is rewritten on disk, even with a slow round", async function () {
    this.timeout(150_000);
    const workspace = path.resolve(__dirname, "../../../test/fixtures/workspace");
    // #2144: no dot prefix, in case the host's watcher skips dot folders.
    const slow = fs.realpathSync(fs.mkdtempSync(path.join(workspace, "ptx-")));
    const slowProject = path.basename(slow);
    try {
      fs.mkdirSync(path.join(slow, "src"));
      fs.writeFileSync(
        path.join(slow, "bynk.toml"),
        '[project]\nname = "ptx"\n\n[paths]\ninclude = ["src"]\n\n[lsp]\ndiagnostics_debounce_ms = 2500\n',
      );
      fs.writeFileSync(path.join(slow, "src", "billing.bynk"), FILES["src/billing.bynk"].replace("uses money\n\n", "").replace("cents(qty)", "qty"));
      fs.writeFileSync(path.join(slow, "src", "storefront.bynk"), FILES["src/storefront.bynk"]);
      const treeState = () => vscode.commands.executeCommand<unknown>("bynk._projectTreeState");
      const shown = async () => {
        // Keep the view visible: a hidden tree defers its reload until shown.
        await vscode.commands.executeCommand("workbench.view.extension.bynk");
        return snapshot(5);
      };
      await vscode.window.showTextDocument(
        await vscode.workspace.openTextDocument(vscode.Uri.file(path.join(slow, "src", "storefront.bynk"))),
      );
      await waitFor(
        async () => {
          await vscode.commands.executeCommand("bynk.project.refresh");
          return shown();
        },
        (t) => find(t, slowProject, "Contexts", "billing", "Consumed by") !== undefined,
        "the slow project's edge to appear",
        60_000,
      );

      // Start from an idle scheduler, so the refresh this rewrite needs has
      // to come from its own trigger. The server's debounce here (2.5 s) is
      // longer than the settle timer (1.5 s), and the project is clean before
      // and after, so no diagnostics publish signals the round: unless a
      // publish happens to land, only the follow-up fires (#2144) see it.
      await waitFor(
        () => vscode.commands.executeCommand<{ hub: { armed: boolean } }>("bynk._projectTreeState"),
        (st) => !st.hub.armed,
        "the scheduler to disarm",
        45_000,
      );
      // Write through VS Code's file system API, not Node's `fs`. Both are
      // outside the editor (no save, no edit), but a Node write is only seen
      // through the OS file watcher, and on CI runners that watcher reported
      // nothing for these test-created folders: neither the watcher event
      // nor the editor's reload of the open file arrived (#2144). VS Code's
      // own write reports the change to watchers directly, so the test
      // exercises the extension's path (event, scheduler, model refresh,
      // tree) rather than the runner's inotify.
      // Close the file first, so the server reads the rewrite from disk rather
      // than keeping the open buffer's text.
      const storefront = vscode.Uri.file(path.join(slow, "src", "storefront.bynk"));
      await vscode.commands.executeCommand("workbench.action.closeAllEditors");
      await vscode.workspace.fs.writeFile(
        storefront,
        new TextEncoder().encode(
          "context storefront\n\nservice checkout {\n  on call(sku: String, qty: Int) -> Effect[Int] {\n    qty\n  }\n}\n",
        ),
      );
      // The file watcher should report the change. On CI runners it has
      // delivered nothing for this test's folder (#2144), which no extension
      // code can work around; there the test skips, saying why.
      let watcherSaw = true;
      try {
        await waitFor(
          () => vscode.commands.executeCommand<boolean>("bynk._sawExternalChange", storefront.toString()),
          (saw) => saw === true,
          "the refresh hub to see the external change to storefront.bynk",
          15_000,
        );
      } catch {
        watcherSaw = false;
      }
      if (!watcherSaw) {
        // Without the event neither the extension nor the language server
        // (whose watched-files notifications come from the same watcher) can
        // know the file changed, so the rest of this test can't be checked
        // here. The scheduler's follow-ups are covered by unit tests.
        console.log(
          "[#2144] skipping: the file watcher delivered no event for the rewrite in this environment",
        );
        this.skip();
      }
      try {
        await waitFor(
          shown,
          (t) =>
            find(t, slowProject, "Contexts", "storefront") !== undefined &&
            find(t, slowProject, "Contexts", "storefront", "Consumes") === undefined &&
            find(t, slowProject, "Contexts", "billing", "Consumed by") === undefined,
          "the edge to disappear after the external rewrite",
          60_000,
        );
      } catch (err) {
        throw new Error(
          `${String(err)}\nwatcher saw the change: ${watcherSaw}\ntree state: ${JSON.stringify(await treeState())}`,
        );
      }
    } finally {
      try {
        fs.rmSync(slow, { recursive: true, force: true });
      } catch {
        /* best-effort */
      }
    }
  });
});
