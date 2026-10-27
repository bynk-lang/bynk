// #2473 integration test: a pending npm requirement reported by a failing
// rebuild reaches the Problems panel with no terminal command, and
// `Bynk: Review Pending npm Requirements…` lists through the driver and opens
// the review. Runs against the stand-in `bynk` (test/fixtures/fake-bynk).

import * as assert from "assert";
import * as vscode from "vscode";

import { useFakeBynk, waitFor } from "./fakeBynk";

const approvalProblems = () =>
  vscode.languages
    .getDiagnostics()
    .flatMap(([, ds]) => ds)
    .filter((d) => d.source === "bynk approve");

function nextTask(
  event: vscode.Event<vscode.TaskStartEvent | vscode.TaskEndEvent>,
): Promise<vscode.TaskExecution> {
  return new Promise((resolve) => {
    const sub = event((e) => {
      if (e.execution.task.definition.type === "bynk") {
        sub.dispose();
        resolve(e.execution);
      }
    });
  });
}

describe("Bynk: Review Pending npm Requirements (#2473)", () => {
  let fake: Awaited<ReturnType<typeof useFakeBynk>>;

  before(async () => {
    fake = await useFakeBynk();
  });

  after(async () => {
    await vscode.commands.executeCommand("workbench.action.closeAllEditors");
    await fake.restore();
  });

  it("finds a pending requirement in a failing rebuild's output and puts it in Problems", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(45_000);
    process.env.BYNK_FAKE_DEV = "rebuilds";
    process.env.BYNK_FAKE_APPROVE = "pending";
    try {
      const started = nextTask(vscode.tasks.onDidStartTask);
      await vscode.commands.executeCommand("bynk.startDevServer");
      await started;
      await waitFor(() => approvalProblems().length === 2, "the pending packages in Problems");
      const messages = approvalProblems().map((d) => d.message);
      assert.ok(messages.some((m) => m.includes("`jose` ^6.0.0")));
      assert.ok(messages.some((m) => m.includes("`left-pad` ^1.3.0")));
      assert.deepStrictEqual(
        fake.read().find((e) => e.argv?.[0] === "approve")?.argv,
        ["approve", "--list", "--format", "json"],
      );
    } finally {
      const ended = nextTask(vscode.tasks.onDidEndTask);
      await vscode.commands.executeCommand("bynk.stopDevServer");
      await ended;
      delete process.env.BYNK_FAKE_DEV;
      delete process.env.BYNK_FAKE_APPROVE;
    }
  });

  it("opens the review through the driver's listing", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(30_000);
    await vscode.commands.executeCommand("bynk.approveRequirements");
    await waitFor(
      () => vscode.window.tabGroups.all.some((g) => g.tabs.some((t) => t.label === "Bynk: npm Requirements")),
      "the review panel",
    );
  });

  it("approves per package, and approve-all only the packages it named", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(45_000);
    const approvals = () =>
      fake
        .read()
        .map((e) => e.argv)
        .filter((a): a is string[] => a?.[0] === "approve" && !a.includes("--list"));
    const panelOpen = () =>
      vscode.window.tabGroups.all.some((g) => g.tabs.some((t) => t.label === "Bynk: npm Requirements"));
    await vscode.commands.executeCommand("bynk.approveRequirements");
    await waitFor(panelOpen, "the review panel");

    // A package the listing does not have pending is ignored.
    await vscode.commands.executeCommand("bynk._approveAction", "approve", { package: "zod" }, false);
    assert.deepStrictEqual(approvals(), []);

    await vscode.commands.executeCommand("bynk._approveAction", "approve", { package: "jose" }, false);
    assert.deepStrictEqual(approvals(), [["approve", "jose"]]);

    // Declining the confirmation approves nothing.
    await vscode.commands.executeCommand("bynk._approveAction", "approveAll", {}, false);
    assert.deepStrictEqual(approvals(), [["approve", "jose"]]);

    // Confirming approves each package it named, by name.
    await vscode.commands.executeCommand("bynk._approveAction", "approveAll", {}, true);
    assert.deepStrictEqual(approvals(), [["approve", "jose"], ["approve", "left-pad"]]);
    assert.ok(!approvals().some((a) => a.includes("--all")), "never `bynk approve --all`");
  });
});
