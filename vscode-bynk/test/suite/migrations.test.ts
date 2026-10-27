// #2468 integration test: the Project sidebar's State Migrations node lists
// each agent class from `bynk migrate status --format json`, and `Bynk: Sweep
// State Migrations…` opens the plan without running a sweep. Runs against the
// stand-in `bynk` (test/fixtures/fake-bynk), which prints the pinned report and
// refuses a real sweep, so nothing here can reach Cloudflare.

import * as assert from "assert";
import * as vscode from "vscode";

import { useFakeBynk, waitFor } from "./fakeBynk";

interface Entry {
  label: string;
  contextValue?: string;
  description?: string;
  children?: Entry[];
}

const snapshot = (depth: number, reload: boolean) =>
  vscode.commands.executeCommand<Entry[]>("bynk._projectTreeSnapshot", depth, reload);

/** Every "State Migrations" node in the tree (one per project). */
function migrationNodes(tree: Entry[]): Entry[] {
  const out: Entry[] = [];
  const walk = (es: Entry[]) => {
    for (const e of es) {
      if (e.contextValue === "bynkMigrations") out.push(e);
      if (e.children) walk(e.children);
    }
  };
  walk(tree);
  return out;
}

function planTabOpen(title: string): boolean {
  return vscode.window.tabGroups.all.some((g) => g.tabs.some((t) => t.label === title));
}

describe("Bynk State Migrations (#2468)", () => {
  let fake: Awaited<ReturnType<typeof useFakeBynk>>;

  before(async function () {
    this.timeout(60_000);
    fake = await useFakeBynk();
    await vscode.extensions.getExtension("bynk-lang.bynk-vscode")?.activate();
  });

  after(async () => {
    await vscode.commands.executeCommand("workbench.action.closeAllEditors");
    await fake.restore();
  });

  it("lists each class with its version, floor and steps", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(90_000);
    const node = migrationNodes(await snapshot(4, true))[0];
    assert.ok(node, "a State Migrations node");
    assert.deepStrictEqual(
      node.children?.map((c) => [c.label, c.description]),
      [
        ["shop.Cart", "v4, floor 1 · 1 live step · versions without a path"],
        ["shop.Order", "v3, floor 2"],
      ],
    );
    const cart = node.children?.[0].children?.map((n) => n.label);
    assert.ok(cart?.some((l) => l.startsWith("migrate from 3")), JSON.stringify(cart));
    assert.ok(cart?.includes("deployed to default, staging"), JSON.stringify(cart));
    const status = fake.read().filter((e) => e.argv?.[0] === "migrate");
    assert.ok(status.length > 0);
    assert.deepStrictEqual(status[0].argv, ["migrate", "status", "--format", "json"]);
  });

  it("opens the sweep plan and never runs a sweep", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(30_000);
    await vscode.commands.executeCommand("bynk.migrate.sweep", { environment: "staging", context: "" });
    await waitFor(() => planTabOpen("Bynk Sweep Plan: staging"), "the sweep plan");
    const runs = fake.read().filter((e) => e.argv?.[0] === "migrate");
    assert.ok(runs.every((r) => r.argv?.[1] === "status"), "only status ran");
    for (const r of runs) assert.ok(!r.argv!.includes("--yes"));
  });
});
