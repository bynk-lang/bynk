// The Test Explorer's Watch toggle (Test Explorer depth track, Slice E, #2269),
// inside the extension host against the stand-in `bynk` (#2094).
//
// Turning Watch on in the real UI goes through a quick pick, so the suite uses the
// `bynk._watchTests` seam, which hands the Run profile's handler a continuous
// request for the whole tree. What the extension ran, and how it stopped it, is read
// from the stand-in's log: each `bynk test` it started, and the SIGTERM it received.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { FAKE_BYNK, useFakeBynk, waitFor, type FakeLogEntry } from "./fakeBynk";

const FIXTURES = path.resolve(__dirname, "../../../test/fixtures");

describe("Watch (continuous test runs)", () => {
  const probe = path.join(FIXTURES, "workspace", "src", "watch_probe.bynk");
  let dir: string;
  let fake: Awaited<ReturnType<typeof useFakeBynk>>;

  /** A document of `n` passing cases, which the stand-in serves for every run. */
  function writeDoc(n: number): void {
    const cases = Array.from({ length: n }, (_, i) => ({
      name: `case ${i}`,
      outcome: "pass",
      location: { path: "src/text.bynk", line: i + 1, col: 1 },
    }));
    const file = path.join(dir, `doc-${n}.json`);
    fs.writeFileSync(file, JSON.stringify({ passed: n, failed: 0, suites: [{ name: "watched", kind: "unit", cases }] }));
    process.env.BYNK_FAKE_TEST_DOC = file;
  }

  /** The log from entry `from`: runs (not discovery) as arguments, and signals. */
  function since(from: number): { runs: string[][]; events: string[] } {
    const log = fake.read().slice(from);
    const runs: string[][] = [];
    const events: string[] = [];
    for (const e of log as FakeLogEntry[]) {
      if (e.signal) events.push(e.signal);
      else if (e.argv?.[0] === "test" && !e.argv.includes("--no-run")) {
        runs.push(e.argv);
        events.push("run");
      }
    }
    return { runs, events };
  }

  /** Save `text` into the probe from the editor, as a user saving the file would. */
  async function editorSave(text: string): Promise<void> {
    const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(probe));
    const edit = new vscode.WorkspaceEdit();
    edit.replace(doc.uri, new vscode.Range(0, 0, doc.lineCount, 0), text);
    await vscode.workspace.applyEdit(edit);
    await doc.save();
  }

  const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

  before(async () => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), "bynk-continuous-"));
    fs.chmodSync(FAKE_BYNK, 0o755);
    fs.writeFileSync(probe, "// watch probe\n");
    fake = await useFakeBynk();
  });
  afterEach(async () => {
    await vscode.commands.executeCommand("bynk._watchTests", false);
    delete process.env.BYNK_FAKE_STREAM_DELAY_MS;
  });
  after(async () => {
    await fake.restore();
    delete process.env.BYNK_FAKE_TEST_DOC;
    fs.rmSync(probe, { force: true });
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it("reruns the whole project once after a save", async function () {
    this.timeout(60_000);
    writeDoc(2);
    await vscode.commands.executeCommand("bynk._watchTests", true);
    const from = fake.read().length;

    await editorSave("// watch probe, saved\n");
    await waitFor(() => since(from).runs.length >= 1, "the rerun after a save");
    // The editor's save and the file watcher both report it; the quiet period folds
    // them into one rerun. Wait well past it before counting.
    await sleep(5_000);

    const { runs } = since(from);
    assert.strictEqual(runs.length, 1, `one rerun for one save, got ${JSON.stringify(runs)}`);
    assert.ok(runs[0].includes("ndjson"), `streamed: ${runs[0]}`);
    assert.ok(!runs[0].includes("--case") && !runs[0].includes("--suite"), `the whole project: ${runs[0]}`);
  });

  it("drops a watcher event for the file just saved, even one that lands after the rerun began", async function () {
    this.timeout(60_000);
    writeDoc(2);
    await vscode.commands.executeCommand("bynk._watchTests", true);
    const from = fake.read().length;

    await editorSave("// saved in the editor\n");
    // The rerun starts a second after the save. A watcher event for the same file
    // inside the hub's echo window (two seconds) is taken for the save's echo and
    // dropped; were it counted, it would cancel the rerun and start another. The
    // quiet period alone cannot hide that: this lands after it has passed.
    await sleep(1_400);
    fs.writeFileSync(probe, "// written by something else, just after\n");
    await sleep(5_000);

    const { runs, events } = since(from);
    assert.strictEqual(runs.length, 1, `one rerun, not two: ${JSON.stringify(events)}`);
    assert.ok(!events.includes("SIGTERM"), `nothing was cancelled: ${JSON.stringify(events)}`);
  });

  it("turns a burst of saves into one rerun", async function () {
    this.timeout(60_000);
    writeDoc(2);
    await vscode.commands.executeCommand("bynk._watchTests", true);
    const from = fake.read().length;

    await editorSave("// burst 1\n");
    await sleep(200);
    await editorSave("// burst 2\n");
    await sleep(200);
    await editorSave("// burst 3\n");
    await waitFor(() => since(from).runs.length >= 1, "the rerun after the burst");
    await sleep(5_000);

    assert.strictEqual(since(from).runs.length, 1, "three saves inside the quiet period are one rerun");
  });

  it("reruns after a change made outside the editor", async function () {
    this.timeout(60_000);
    writeDoc(2);
    await vscode.commands.executeCommand("bynk._watchTests", true);
    const from = fake.read().length;

    fs.writeFileSync(probe, "// changed on disk\n");
    await waitFor(() => since(from).runs.length >= 1, "the rerun after an external change");
    await sleep(3_000);

    assert.strictEqual(since(from).runs.length, 1);
  });

  it("does nothing on a change once Watch is off", async function () {
    this.timeout(60_000);
    writeDoc(2);
    await vscode.commands.executeCommand("bynk._watchTests", true);
    await vscode.commands.executeCommand("bynk._watchTests", false);
    const from = fake.read().length;

    await editorSave("// nobody is watching\n");
    await sleep(4_000);

    assert.strictEqual(since(from).runs.length, 0);
  });

  it("stops the run in flight when a change arrives, and starts the next only once it has ended", async function () {
    this.timeout(90_000);
    // Four cases, each held for 2.5 s, so a run takes about ten seconds.
    writeDoc(4);
    process.env.BYNK_FAKE_STREAM_DELAY_MS = "2500";
    await vscode.commands.executeCommand("bynk._watchTests", true);
    const from = fake.read().length;

    await editorSave("// first change\n");
    await waitFor(() => since(from).events.includes("run"), "the first run to start");
    await sleep(1_000); // it is now in flight
    await editorSave("// second change, during the run\n");

    await waitFor(() => since(from).runs.length >= 2, "the replacement run", 30_000);
    // The cancelled run was killed, and only then did the next one start.
    assert.deepStrictEqual(since(from).events.slice(0, 3), ["run", "SIGTERM", "run"]);
  });
});
