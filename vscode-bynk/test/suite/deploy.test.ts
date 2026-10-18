// #2101 integration test: `Bynk: Show Deploy Plan` runs only the dry run and
// opens the plan view. Runs against the stand-in `bynk`
// (test/fixtures/fake-bynk), which prints the pinned plan for a dry run and
// refuses a real deploy, so nothing here can reach Cloudflare.

import * as assert from "assert";
import * as vscode from "vscode";

import { useFakeBynk, waitFor } from "./fakeBynk";

function planTabOpen(env: string): boolean {
  return vscode.window.tabGroups.all.some((g) =>
    g.tabs.some((t) => t.label === `Bynk Deploy Plan: ${env}`),
  );
}

describe("Bynk deploy plan (#2101)", () => {
  let fake: Awaited<ReturnType<typeof useFakeBynk>>;

  before(async () => {
    fake = await useFakeBynk();
  });

  after(async () => {
    await vscode.commands.executeCommand("workbench.action.closeAllEditors");
    await fake.restore();
  });

  it("previews with --dry-run and never runs a deploy", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(30_000);
    await vscode.commands.executeCommand("bynk.showDeployPlan", { environment: "staging", context: "" });
    await waitFor(() => planTabOpen("staging"), "the plan view");

    const runs = fake.read().filter((e) => e.argv?.[0] === "deploy");
    assert.strictEqual(runs.length, 1);
    assert.deepStrictEqual(runs[0].argv, ["deploy", "--dry-run", "--format", "json", "--env", "staging"]);
    for (const banned of ["--yes", "--prune"]) {
      assert.ok(!runs[0].argv!.includes(banned), banned);
    }
  });
});
