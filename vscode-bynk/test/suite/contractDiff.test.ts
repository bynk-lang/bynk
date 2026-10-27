// #2467: "Bynk: Show Contract Diff" against the real language server. The
// diff's content (compatible and breaking cases) is covered where it is
// computed (`bynk-lsp`'s `contract_diff_classifies_boundary_changes_against_the_lock`);
// this proves the command reaches the server and opens a populated panel for
// a project whose lock a source change has broken.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import * as vscode from "vscode";

const EXT_ID = "bynk-lang.bynk-vscode";
const VIEW = "bynkContractDiff";

const SRC = `context commerce.order

consumes bynk { Events }

event ShipmentSent = {
  orderId: String,
}

service ship {
  on call(orderId: String) -> Effect[()] given Events {
    Events.emit[ShipmentSent](ShipmentSent { orderId: orderId })
  }
}
`;

// \`email\` was locked and has since been removed: a breaking change.
const LOCK = `version = 2

[events]

[events."commerce.order.ShipmentSent"]
schema = 1

[[events."commerce.order.ShipmentSent".fields]]
name = "orderId"
type = "String"
default = false

[[events."commerce.order.ShipmentSent".fields]]
name = "email"
type = "String"
default = false
`;

interface Snapshot {
  open: boolean;
  hasPayload: boolean;
  nullRefreshes: number;
}

async function waitFor<T>(fn: () => Thenable<T> | T, ok: (v: T) => boolean, what: string, timeoutMs = 60_000): Promise<T> {
  const start = Date.now();
  for (;;) {
    const v = await fn();
    if (ok(v)) return v;
    if (Date.now() - start > timeoutMs) throw new Error(`timed out waiting for ${what}: ${JSON.stringify(v)}`);
    await new Promise((r) => setTimeout(r, 200));
  }
}

describe("contract diff view (#2467)", () => {
  let dir: string;

  before(async function () {
    this.timeout(120_000);
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    await ext.activate();
    const workspace = path.resolve(__dirname, "../../../test/fixtures/workspace");
    dir = fs.realpathSync(fs.mkdtempSync(path.join(workspace, ".cd-")));
    fs.mkdirSync(path.join(dir, "src", "commerce"), { recursive: true });
    fs.writeFileSync(path.join(dir, "bynk.toml"), '[project]\nname = "cd"\n\n[paths]\ninclude = ["src"]\n');
    fs.writeFileSync(path.join(dir, "src", "commerce", "order.bynk"), SRC);
    fs.writeFileSync(path.join(dir, "bynk.schema.lock"), LOCK);
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

  it("is a contributed command that opens a populated panel", async function () {
    this.timeout(120_000);
    const ext = vscode.extensions.getExtension(EXT_ID);
    const commands: Array<{ command: string }> = ext?.packageJSON.contributes.commands ?? [];
    assert.ok(commands.some((c) => c.command === "bynk.showContractDiff"), "the command is contributed");

    const uri = vscode.Uri.file(path.join(dir, "src", "commerce", "order.bynk"));
    await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(uri));
    // The server commits its first round for the new project asynchronously.
    const snap = await waitFor(
      async () => {
        await vscode.commands.executeCommand("bynk.showContractDiff", uri.toString());
        return vscode.commands.executeCommand<Snapshot>("bynk._liveViewState", VIEW);
      },
      (s) => s?.open === true && s.hasPayload,
      "the contract diff to open with a payload",
    );
    assert.ok(snap.hasPayload);
  });
});
