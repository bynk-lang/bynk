// Debugging a selection as a set (Test Explorer depth track, Slice A follow-up),
// inside the extension host against the stand-in `bynk` (#2094): the launch passes
// the set to `bynk test --inspect`, and a driver that predates the set filter is
// debugged unfiltered instead. The stand-in has no inspector, so what is checked is
// the launch's arguments, read from its log.

import * as assert from "assert";
import * as fs from "fs";
import * as vscode from "vscode";

import { FAKE_BYNK, useFakeBynk, waitFor, type FakeLogEntry } from "./fakeBynk";

describe("Debugging a set of tests", () => {
  let fake: Awaited<ReturnType<typeof useFakeBynk>>;

  const inspects = (from: number): string[][] =>
    fake
      .read()
      .slice(from)
      .map((e: FakeLogEntry) => e.argv ?? [])
      .filter((a) => a[0] === "test" && a.includes("--inspect"));

  before(() => fs.chmodSync(FAKE_BYNK, 0o755));
  beforeEach(async () => {
    fake = await useFakeBynk();
  });
  afterEach(async () => {
    delete process.env.BYNK_FAKE_SETS;
    // The extension forgets what it learned about a driver when the binary setting
    // changes: point it elsewhere and back (the bundled extension keeps its own state).
    const config = vscode.workspace.getConfiguration("bynk");
    await config.update("bynkPath", "/nonexistent/bynk", vscode.ConfigurationTarget.Global);
    await fake.restore();
  });

  const launch = (extra: Record<string, unknown>) =>
    vscode.debug.startDebugging(vscode.workspace.workspaceFolders?.[0] ?? undefined, {
      type: "bynk",
      request: "launch",
      name: "Debug selected Bynk tests",
      mode: "test",
      ...extra,
    });

  it("passes the set to the inspected run", async function () {
    this.timeout(60_000);
    const from = fake.read().length;
    await launch({ suites: ["text"], cases: ["one", "two"] });
    await waitFor(() => inspects(from).length >= 1, "the inspected run");
    const argv = inspects(from)[0];
    assert.deepStrictEqual(argv.slice(argv.indexOf("--inspect") + 1), [
      "--suite",
      "text",
      "--case",
      "one",
      "--case",
      "two",
    ]);
  });

  it("debugs the whole project against a driver that cannot take a set", async function () {
    this.timeout(60_000);
    process.env.BYNK_FAKE_SETS = "unsupported";
    const from = fake.read().length;
    await launch({ suites: ["text"], cases: ["one", "two"] });
    await waitFor(() => inspects(from).length >= 2, "the unfiltered retry");
    const [first, second] = inspects(from);
    assert.ok(first.includes("--suite"), `the set was tried first: ${JSON.stringify(first)}`);
    assert.deepStrictEqual(second.slice(second.indexOf("--inspect") + 1), [], "then the whole project");
  });
});
