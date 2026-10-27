// #2094 integration test: `Bynk: Start Dev Server` runs `bynk dev --format json`
// as the `bynk: dev` task, and `Bynk: Stop Dev Server` ends it with a SIGTERM to
// its process group. Runs against the stand-in `bynk` (test/fixtures/fake-bynk),
// so it needs no wrangler and serves nothing real.

import * as assert from "assert";
import * as vscode from "vscode";

import { useFakeBynk, waitFor } from "./fakeBynk";

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

describe("Bynk dev server (#2094)", () => {
  let fake: Awaited<ReturnType<typeof useFakeBynk>>;

  before(async () => {
    fake = await useFakeBynk();
  });

  after(async () => {
    await vscode.commands.executeCommand("bynk.stopDevServer");
    await fake.restore();
  });

  it("contributes the bynk: dev task", async () => {
    const tasks = await vscode.tasks.fetchTasks({ type: "bynk" });
    const dev = tasks.find((t) => t.definition.command === "dev");
    assert.ok(dev, "a bynk: dev task is provided");
    assert.strictEqual(dev.isBackground, true);
  });

  it("starts `bynk dev --format json` and stops its process group", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(30_000);

    const started = nextTask(vscode.tasks.onDidStartTask);
    await vscode.commands.executeCommand("bynk.startDevServer");
    const execution = await started;
    assert.strictEqual(execution.task.definition.command, "dev");

    await waitFor(() => fake.read().some((e) => e.argv?.[0] === "dev"), "the stand-in to start");
    const argv = fake.read().find((e) => e.argv?.[0] === "dev")!.argv!;
    assert.deepStrictEqual(argv.slice(0, 3), ["dev", "--format", "json"]);

    const ended = nextTask(vscode.tasks.onDidEndTask);
    await vscode.commands.executeCommand("bynk.stopDevServer");
    await ended;
    assert.ok(
      fake.read().some((e) => e.signal === "SIGTERM"),
      "Stop sends SIGTERM, which `bynk dev` traps to tear down every worker",
    );
  });

  it("puts a failing rebuild in Problems and clears it on the next good one", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(45_000);
    const devProblems = () =>
      vscode.languages
        .getDiagnostics()
        .flatMap(([, ds]) => ds)
        .filter((d) => d.source === "bynk dev");
    // The stand-in emits `rebuild_failed` after 1s and `rebuild_ok` after 5s,
    // and its `check` reports one error meanwhile.
    process.env.BYNK_FAKE_DEV = "rebuilds";
    try {
      const started = nextTask(vscode.tasks.onDidStartTask);
      await vscode.commands.executeCommand("bynk.startDevServer");
      await started;
      await waitFor(() => devProblems().length > 0, "the rebuild error in Problems");
      const [diag] = devProblems();
      assert.strictEqual(diag.message, "fake rebuild error");
      assert.strictEqual(diag.code, "bynk.fake.rebuild");
      await waitFor(() => devProblems().length === 0, "the good rebuild to clear Problems");
      assert.ok(
        fake.read().some((e) => e.argv?.join(" ") === "check . --format short"),
        "the failing rebuild ran `bynk check . --format short`",
      );
    } finally {
      const ended = nextTask(vscode.tasks.onDidEndTask);
      await vscode.commands.executeCommand("bynk.stopDevServer");
      await ended;
      delete process.env.BYNK_FAKE_DEV;
    }
  });

  it("shows the workers' logs, with .bynk frames, filterable by level and context (#2474)", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(45_000);
    const lines = () => vscode.commands.executeCommand<string[]>("bynk.logLines").then((l) => l ?? []);
    await vscode.commands.executeCommand("bynk.clearLogs");
    process.env.BYNK_FAKE_DEV = "logs";
    try {
      const started = nextTask(vscode.tasks.onDidStartTask);
      await vscode.commands.executeCommand("bynk.startDevServer");
      await started;
      assert.ok(
        fake.read().some((e) => e.argv?.includes("--log-events")),
        "the session asks the driver for log events",
      );
      await waitFor(
        async () => (await lines()).some((l) => l.includes("slow handler")),
        "the worker logs in the view",
      );
      const shown = await lines();
      assert.ok(shown.some((l) => l === "[fixture] Error: boom"), "a headline is tagged with its context");
      assert.ok(
        shown.some((l) => l === "[fixture]     at http_GET (/work/src/shop.bynk:12:3)"),
        "a frame keeps its .bynk location, for the output channel to link",
      );

      await vscode.commands.executeCommand("bynk.filterLogs", { minLevel: "error" });
      const errors = await lines();
      assert.ok(errors.every((l) => !l.includes("GET / 500") && !l.includes("slow handler")));
      assert.ok(errors.some((l) => l.includes("shop.bynk:12:3")), "the errors stay");

      await vscode.commands.executeCommand("bynk.filterLogs", { minLevel: "info", contexts: ["other"] });
      const other = (await lines()).filter((l) => !l.includes("--- bynk dev"));
      assert.deepStrictEqual(other, ["[other] ▲ [WARNING] slow handler"]);

      await vscode.commands.executeCommand("bynk.filterLogs", { minLevel: "info" });
      assert.ok((await lines()).length >= 4, "clearing the filter shows them all again");

      await vscode.commands.executeCommand("bynk.clearLogs");
      assert.deepStrictEqual(await lines(), [], "Clear empties the buffer");
    } finally {
      const ended = nextTask(vscode.tasks.onDidEndTask);
      await vscode.commands.executeCommand("bynk.stopDevServer");
      await ended;
      delete process.env.BYNK_FAKE_DEV;
    }
  });
});
