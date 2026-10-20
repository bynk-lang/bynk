// #2196: the agent state view end to end, against the real language server —
// the `Show State` lens, the agent-scoped cursor-follow (retarget per agent,
// highlight per handler without a re-request) and the empty state. State is
// read through the `bynk._liveViewState` test seam, as in `liveViews.test.ts`.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import * as vscode from "vscode";

const EXT_ID = "bynk.bynk-vscode";
const VIEW = "bynkAgentState";

interface Snapshot {
  open: boolean;
  target: { uri: string; position?: { line: number; character: number } } | undefined;
  handler: string | undefined;
  refreshes: number;
  updates: number;
  hasPayload: boolean;
  focus: string | undefined;
}

const SRC = `context orders

type OrderStatus = enum { Pending, Placed, Cancelled }
type OrderError = enum { AlreadyPlaced, AlreadyCancelled }

agent Order {
  key id: String

  store status: Cell[OrderStatus] = Pending

  on call place() -> Effect[Result[(), OrderError]] {
    match status {
      Pending => {
        status := Placed
        Ok(())
      }
      Placed    => Err(AlreadyPlaced)
      Cancelled => Err(AlreadyCancelled)
    }
  }

  on call cancel() -> Effect[Result[(), OrderError]] {
    match status {
      Pending => {
        status := Cancelled
        Ok(())
      }
      Placed    => Err(AlreadyPlaced)
      Cancelled => Err(AlreadyCancelled)
    }
  }
}

agent Counter {
  key id: String

  store total: Cell[Int]

  on call bump() -> Effect[Int] {
    total := total + 1
    total
  }
}
`;

function state(): Thenable<Snapshot> {
  return vscode.commands.executeCommand<Snapshot>("bynk._liveViewState", VIEW);
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

describe("agent state view (#2196)", () => {
  let dir: string;
  let uri: vscode.Uri;
  let editor: vscode.TextEditor;

  before(async function () {
    this.timeout(120_000);
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    await ext.activate();

    const workspace = path.resolve(__dirname, "../../../test/fixtures/workspace");
    dir = fs.realpathSync(fs.mkdtempSync(path.join(workspace, ".as-")));
    fs.mkdirSync(path.join(dir, "src"));
    fs.writeFileSync(path.join(dir, "bynk.toml"), '[project]\nname = "as"\n\n[paths]\ninclude = ["src"]\n');
    fs.writeFileSync(path.join(dir, "src", "orders.bynk"), SRC);
    uri = vscode.Uri.file(path.join(dir, "src", "orders.bynk"));
    editor = await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(uri));

    // Open on `place`'s body once the server has committed a round.
    const inPlace = new vscode.Position(lineOf("status := Placed"), 8);
    await waitFor(
      async () => {
        await vscode.commands.executeCommand("bynk.showAgentState", uri.toString(), inPlace);
        return state();
      },
      (s) => s?.open === true && s.hasPayload,
      "the agent state view to open",
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

  it("offers one Show State lens per agent", async function () {
    this.timeout(30_000);
    const lenses = await waitFor(
      async () =>
        ((await vscode.commands.executeCommand<vscode.CodeLens[]>("vscode.executeCodeLensProvider", uri)) ?? [])
          .filter((l) => l.command?.command === "bynk.showAgentState"),
      (l) => l.length >= 2,
      "the Show State lenses",
    );
    assert.deepStrictEqual(
      lenses.map((l) => l.range.start.line).sort((a, b) => a - b),
      [lineOf("agent Order"), lineOf("agent Counter")],
    );
  });

  it("targets the agent and highlights the handler the command was run in", async () => {
    const s = await state();
    assert.ok(s.handler?.includes("#Order@"), `keyed on the agent: ${s.handler}`);
    assert.strictEqual(s.focus, "place");
  });

  it("moves the highlight within the agent without re-requesting its model", async function () {
    this.timeout(30_000);
    editor = await vscode.window.showTextDocument(editor.document, vscode.ViewColumn.One);
    const before = await state();
    const inCancel = new vscode.Position(lineOf("status := Cancelled"), 8);
    editor.selection = new vscode.Selection(inCancel, inCancel);
    const after = await waitFor(() => state(), (s) => s.focus === "cancel", "the highlight to move");
    assert.strictEqual(after.handler, before.handler, "still the same agent");
    assert.strictEqual(after.updates, before.updates, "no new model was rendered");
  });

  it("retargets to another agent, which has no state field", async function () {
    this.timeout(30_000);
    const before = await state();
    const inBump = new vscode.Position(lineOf("total := total + 1"), 6);
    editor.selection = new vscode.Selection(inBump, inBump);
    const after = await waitFor(() => state(), (s) => s.handler !== before.handler, "a retarget to Counter");
    assert.ok(after.handler?.includes("#Counter@"), `keyed on Counter: ${after.handler}`);
    assert.ok(after.hasPayload, "an agent with no state field still has a (machine-free) model");
    assert.strictEqual(after.focus, "bump");
  });
});
