// #2095 integration test: `Bynk: Doctor` runs `bynk doctor --format json` and
// opens the checklist, scoped with `--only` when asked. Runs against the
// stand-in `bynk` (test/fixtures/fake-bynk), which prints the driver's golden
// report and runs no probe.

import * as assert from "assert";
import * as vscode from "vscode";

import { useFakeBynk, waitFor } from "./fakeBynk";

function doctorTabOpen(): boolean {
  return vscode.window.tabGroups.all.some((g) => g.tabs.some((t) => t.label === "Bynk Doctor"));
}

describe("Bynk: Doctor (#2095)", () => {
  let fake: Awaited<ReturnType<typeof useFakeBynk>>;

  before(async () => {
    fake = await useFakeBynk();
  });

  after(async () => {
    await vscode.commands.executeCommand("workbench.action.closeAllEditors");
    await fake.restore();
  });

  it("runs `bynk doctor --format json`, scoped with --only, and shows the report", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(30_000);
    await vscode.commands.executeCommand("bynk.doctor", { only: "test" });
    await waitFor(doctorTabOpen, "the doctor view");
    const run = fake.read().find((e) => e.argv?.[0] === "doctor");
    assert.deepStrictEqual(run?.argv, ["doctor", "--format", "json", "--only", "test", "."]);
  });
});
