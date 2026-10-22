// The Test Explorer's streaming run (Test Explorer depth track, Slice D stage 1),
// inside the extension host against the stand-in `bynk` (#2094): the run asks for
// `--format ndjson`, each suite reaches the tree while the process is still
// running, and a driver that predates the stream is run as `json` instead.
//
// The stand-in writes the driver's event stream for a document, and
// `BYNK_FAKE_STREAM_DELAY_MS` holds it after each suite so the order is visible.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { runBynkcTestStream, streamState } from "../../src/testing";
import type { JsonSuite } from "../../src/testPlan";
import { FAKE_BYNK, useFakeBynk, waitFor, type FakeLogEntry } from "./fakeBynk";

const FIXTURES = path.resolve(__dirname, "../../../test/fixtures");

describe("Streaming test results", () => {
  const workspace = vscode.Uri.file(path.resolve(FIXTURES, "workspace"));
  let dir: string;
  let fake: Awaited<ReturnType<typeof useFakeBynk>>;

  const twoSuites = {
    passed: 2,
    failed: 0,
    suites: [
      { name: "first", kind: "unit", cases: [{ name: "a", outcome: "pass", tier: "unit" }] },
      { name: "second", kind: "unit", cases: [{ name: "b", outcome: "pass", tier: "unit" }] },
    ],
  };

  function tests(from: number): string[][] {
    return fake
      .read()
      .slice(from)
      .map((e: FakeLogEntry) => e.argv ?? [])
      .filter((a) => a[0] === "test");
  }

  before(() => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), "bynk-stream-"));
    fs.chmodSync(FAKE_BYNK, 0o755);
    fs.writeFileSync(path.join(dir, "two.json"), JSON.stringify(twoSuites));
  });

  beforeEach(async () => {
    fake = await useFakeBynk();
    process.env.BYNK_FAKE_TEST_DOC = path.join(dir, "two.json");
    streamState.ndjson = true;
  });

  afterEach(async () => {
    delete process.env.BYNK_FAKE_TEST_DOC;
    delete process.env.BYNK_FAKE_STREAM_DELAY_MS;
    delete process.env.BYNK_FAKE_NDJSON;
    streamState.ndjson = true;
    await fake.restore();
  });

  after(() => fs.rmSync(dir, { recursive: true, force: true }));

  it("hands over each suite while the run is still going", async function () {
    this.timeout(30_000);
    process.env.BYNK_FAKE_STREAM_DELAY_MS = "1500";
    const token = new vscode.CancellationTokenSource();
    const started = Date.now();
    const arrivals: { name: string; at: number }[] = [];
    const { doc, streamed } = await runBynkcTestStream(workspace, token.token, {}, (suite: JsonSuite) =>
      arrivals.push({ name: suite.name, at: Date.now() - started }),
    );
    const total = Date.now() - started;
    assert.ok(streamed);
    assert.deepStrictEqual(
      arrivals.map((a) => a.name),
      ["first", "second"],
    );
    assert.ok(
      arrivals[0].at < total - 1000,
      `the first suite arrived at ${arrivals[0].at}ms, well before the run ended at ${total}ms`,
    );
    assert.strictEqual(doc.passed, 2);
    assert.strictEqual(doc.suites?.length, 2);
    assert.strictEqual(doc.error, undefined);
    assert.ok(tests(0)[0].includes("ndjson"), `asked for the stream: ${tests(0)[0]}`);
    token.dispose();
  });

  it("treats a stopped run as cancelled, not as a runner that died", async function () {
    this.timeout(30_000);
    process.env.BYNK_FAKE_STREAM_DELAY_MS = "4000";
    const token = new vscode.CancellationTokenSource();
    const started = Date.now();
    let outcome: "resolved" | "rejected" = "resolved";
    let reason = "";
    await runBynkcTestStream(workspace, token.token, {}, () => {
      // The first suite has arrived and the run is held; the user presses Stop.
      token.cancel();
    }).then(
      () => undefined,
      (e) => {
        outcome = "rejected";
        reason = String(e);
      },
    );
    assert.strictEqual(outcome, "rejected", "a cancelled run does not resolve with a crash document");
    assert.ok(/cancelled/.test(reason), reason);
    assert.ok(Date.now() - started < 3500, "the process was killed, not waited for");
    token.dispose();
  });

  it("reads a failure to compile from the stream", async function () {
    this.timeout(30_000);
    process.env.BYNK_FAKE_AGAINST = "unsupported";
    try {
      const token = new vscode.CancellationTokenSource();
      const { doc, streamed } = await runBynkcTestStream(workspace, token.token, { against: "dev" }, () => {
        assert.fail("a compile failure holds no suite");
      });
      assert.ok(streamed);
      assert.strictEqual(doc.error?.kind, "compile");
      assert.ok(doc.error?.diagnostics?.[0].includes("advance Clock"));
      token.dispose();
    } finally {
      delete process.env.BYNK_FAKE_AGAINST;
    }
  });

  it("runs a driver that does not know the stream as the one document, and remembers it", async function () {
    this.timeout(60_000);
    process.env.BYNK_FAKE_NDJSON = "unsupported";
    const token = new vscode.CancellationTokenSource();
    const { doc, streamed } = await runBynkcTestStream(workspace, token.token, {}, () => {
      assert.fail("a refused stream holds no suite");
    });
    assert.strictEqual(streamed, false, "the suites are for the caller to apply");
    assert.strictEqual(doc.suites?.length, 2);
    assert.strictEqual(streamState.ndjson, false);
    const formats = tests(0).map((a) => a[a.indexOf("--format") + 1]);
    assert.deepStrictEqual(formats, ["ndjson", "json"]);
    token.dispose();
  });

  it("asks the controller's runs for the stream", async function () {
    this.timeout(60_000);
    const text = vscode.Uri.file(path.join(workspace.fsPath, "src", "text.bynk"));
    await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(text));
    await vscode.commands.executeCommand("testing.refreshTests");
    const from = fake.read().length;
    await vscode.commands.executeCommand("testing.runAll");
    await waitFor(() => tests(from).some((a) => !a.includes("--no-run")), "the run");
    const run = tests(from).find((a) => !a.includes("--no-run")) as string[];
    assert.strictEqual(run[run.indexOf("--format") + 1], "ndjson");
  });
});
