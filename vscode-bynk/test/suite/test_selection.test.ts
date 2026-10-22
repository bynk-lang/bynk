// The Test Explorer's selection and outcome handling (Test Explorer depth track,
// Slice A), inside the extension host.
//
// Two halves:
//   - `applyDocument` with a recording stand-in for the run: a `skipped` case is
//     skipped (not failed), a run keeps to the cases it was asked for, and an
//     outcome it does not know is a named failure. The remote document is shaped
//     after the driver's own `TestRun::with_remote` (`bynk-driver/src/test_json.rs`),
//     field for field; it is not recorded from a live deployment.
//   - "Run Tests in This File" against the stand-in `bynk` (#2094): the processes
//     the extension runs are exactly the plan's, read from the stand-in's log.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { applyDocument, type RunSink } from "../../src/testing";
import { caseKey, planRun, type PlanCase, type TestRun } from "../../src/testPlan";
import { FAKE_BYNK, useFakeBynk, waitFor, type FakeLogEntry } from "./fakeBynk";

const FIXTURES = path.resolve(__dirname, "../../../test/fixtures");

/** A `TestRun` stand-in that records what it was told, in order. */
function recorder(): { sink: RunSink; calls: string[]; output: string[] } {
  const calls: string[] = [];
  const output: string[] = [];
  const sink: RunSink = {
    started: (t) => void calls.push(`started ${t.id}`),
    passed: (t) => void calls.push(`passed ${t.id}`),
    failed: (t, m) => {
      const first = Array.isArray(m) ? m[0] : m;
      const text = typeof first.message === "string" ? first.message : first.message.value;
      calls.push(`failed ${t.id}: ${text}`);
    },
    skipped: (t) => void calls.push(`skipped ${t.id}`),
    appendOutput: (o) => void output.push(o),
  };
  return { sink, calls, output };
}

describe("applyDocument", () => {
  const ROOT = vscode.Uri.file(path.join(os.tmpdir(), "bynk-selection-root"));
  let ctrl: vscode.TestController;

  before(() => {
    ctrl = vscode.tests.createTestController("bynk-selection-test", "Selection test");
  });
  after(() => ctrl.dispose());

  /** Build the tree items for `cases` and return them keyed as the controller does. */
  function tree(cases: { suiteId: string; name: string }[]): Map<string, vscode.TestItem> {
    const items = new Map<string, vscode.TestItem>();
    const suites = new Map<string, vscode.TestItem>();
    for (const c of cases) {
      let suite = suites.get(c.suiteId);
      if (!suite) {
        suite = ctrl.createTestItem(c.suiteId, c.suiteId);
        suites.set(c.suiteId, suite);
        ctrl.items.add(suite);
      }
      const item = ctrl.createTestItem(caseKey(c.suiteId, c.name), c.name);
      suite.children.add(item);
      items.set(item.id, item);
    }
    return items;
  }

  const SHOP = "bynk-test:integration:shop";

  it("reports a skipped case as skipped, with the driver's reason, not as a failure", () => {
    const doc = JSON.parse(fs.readFileSync(path.join(FIXTURES, "test-run-remote.json"), "utf8")) as TestRun;
    const items = tree([
      { suiteId: SHOP, name: "places an order" },
      { suiteId: SHOP, name: "expires a cart" },
    ]);
    const all: PlanCase[] = [...items.keys()].map((k) => ({
      suiteId: SHOP,
      name: k.slice(k.indexOf("::") + 2),
    }));
    const plan = planRun(all, {});
    const { sink, calls, output } = recorder();

    const reported = applyDocument(sink, items, plan, doc, ROOT);

    assert.deepStrictEqual(calls, [
      `started ${caseKey(SHOP, "places an order")}`,
      `passed ${caseKey(SHOP, "places an order")}`,
      `started ${caseKey(SHOP, "expires a cart")}`,
      `skipped ${caseKey(SHOP, "expires a cart")}`,
    ]);
    assert.ok(!calls.some((c) => c.startsWith("failed")), "nothing is reported as failed");
    assert.strictEqual(reported.size, 2);
    const text = output.join("");
    assert.ok(text.includes("skipped: expires a cart — `advance Clock` cannot be honoured"), text);
    assert.ok(text.includes("places an order: retried 2 times"), text);
  });

  it("reports only the requested cases when a whole-project run stands in for a selection", () => {
    const U = "bynk-test:unit:orders";
    const I = "bynk-test:integration:orders";
    const items = tree([
      { suiteId: U, name: "joins" },
      { suiteId: I, name: "joins" },
      { suiteId: U, name: "empty" },
    ]);
    const all: PlanCase[] = [
      { suiteId: U, name: "joins" },
      { suiteId: I, name: "joins" },
      { suiteId: U, name: "empty" },
    ];
    // Ask for the unit `joins` only; `--case joins` runs the integration one too.
    const plan = planRun(all, { include: [{ suiteId: U, name: "joins" }] });
    const doc: TestRun = {
      passed: 2,
      failed: 1,
      suites: [
        { name: "orders", kind: "unit", cases: [{ name: "joins", outcome: "pass" }] },
        { name: "orders", kind: "integration", cases: [{ name: "joins", outcome: "fail", message: "boom" }] },
      ],
    };
    const { sink, calls } = recorder();

    const reported = applyDocument(sink, items, plan, doc, ROOT);

    assert.deepStrictEqual(calls, [`started ${caseKey(U, "joins")}`, `passed ${caseKey(U, "joins")}`]);
    assert.deepStrictEqual([...reported], [caseKey(U, "joins")]);
  });

  it("reports an outcome it does not know as a failure that names it", () => {
    const S = "bynk-test:unit:odd";
    const items = tree([{ suiteId: S, name: "weird" }]);
    const plan = planRun([{ suiteId: S, name: "weird" }], {});
    const doc: TestRun = {
      passed: 0,
      failed: 0,
      suites: [{ name: "odd", kind: "unit", cases: [{ name: "weird", outcome: "flaky", message: "x" }] }],
    };
    const { sink, calls } = recorder();

    applyDocument(sink, items, plan, doc, ROOT);

    assert.strictEqual(calls.length, 2);
    assert.ok(calls[1].startsWith(`failed ${caseKey(S, "weird")}: Unexpected outcome "flaky"`), calls[1]);
  });

  it("creates an item for a case an unnarrowed run returns that the tree has not seen", () => {
    const S = "bynk-test:unit:fresh";
    const items = new Map<string, vscode.TestItem>();
    const plan = planRun([], {});
    const doc: TestRun = {
      passed: 1,
      failed: 0,
      suites: [{ name: "fresh", kind: "unit", cases: [{ name: "new one", outcome: "pass" }] }],
    };
    const { sink, calls } = recorder();
    let created = 0;

    applyDocument(sink, items, { wanted: plan.wanted, narrowed: false }, doc, ROOT, (suite, c) => {
      created++;
      return ctrl.createTestItem(caseKey(`bynk-test:${suite.kind}:${suite.name}`, c.name), c.name);
    });

    assert.strictEqual(created, 1);
    assert.deepStrictEqual(calls, [`started ${caseKey(S, "new one")}`, `passed ${caseKey(S, "new one")}`]);
  });
});

describe("Run Tests in This File", () => {
  const workspace = path.resolve(FIXTURES, "workspace");
  const textFile = vscode.Uri.file(path.join(workspace, "src", "text.bynk"));
  let dir: string;
  let fake: Awaited<ReturnType<typeof useFakeBynk>>;

  /** A discovery/run document whose `names` cases are declared in `src/text.bynk`.
   *  With `elsewhere`, one more case is declared in another file, so the first
   *  file's cases are a strict subset of the tree (a selection that narrows the
   *  run); without it the file holds every test in the project. */
  function writeDoc(names: string[], elsewhere: boolean, split = false): string {
    const suites = [
      {
        name: "text",
        kind: "unit",
        cases: [
          ...names.map((name, i) => ({
            name,
            outcome: "pass",
            location: { path: "src/text.bynk", line: i + 1, col: 1 },
          })),
          // `split`: the suite has one more case, declared in another file, so the
          // file's cases are only part of the suite.
          ...(split
            ? [{ name: "in another file", outcome: "pass", location: { path: "src/other.bynk", line: 2, col: 1 } }]
            : []),
        ],
      },
    ];
    if (elsewhere) {
      suites.push({
        name: "other",
        kind: "unit",
        cases: [{ name: "elsewhere", outcome: "pass", location: { path: "src/other.bynk", line: 1, col: 1 } }],
      });
    }
    const doc = { passed: names.length, failed: 0, suites };
    const file = path.join(dir, `doc-${names.length}-${elsewhere ? "sub" : "all"}${split ? "-split" : ""}.json`);
    fs.writeFileSync(file, JSON.stringify(doc));
    return file;
  }

  before(async () => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), "bynk-selection-"));
    fs.chmodSync(FAKE_BYNK, 0o755);
    fake = await useFakeBynk();
  });
  afterEach(async () => {
    delete process.env.BYNK_FAKE_SETS;
    // The extension forgets what it learned about a driver when the binary setting
    // changes: point it elsewhere and back, since the bundled extension keeps its own
    // copy of that state.
    const config = vscode.workspace.getConfiguration("bynk");
    await config.update("bynkPath", "/nonexistent/bynk", vscode.ConfigurationTarget.Global);
    await config.update("bynkPath", FAKE_BYNK, vscode.ConfigurationTarget.Global);
  });
  after(async () => {
    await fake.restore();
    delete process.env.BYNK_FAKE_TEST_DOC;
    fs.rmSync(dir, { recursive: true, force: true });
  });

  /** Make the tree hold `names`: point the stand-in at their document, refresh,
   *  and wait until the file's run lenses show them. Each scenario must use names
   *  the previous one did not, or the wait can be satisfied by the old tree. */
  async function discover(names: string[], elsewhere = true, split = false): Promise<void> {
    process.env.BYNK_FAKE_TEST_DOC = writeDoc(names, elsewhere, split);
    await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(textFile));
    await vscode.commands.executeCommand("testing.refreshTests");
    const deadline = Date.now() + 45_000;
    for (;;) {
      const lenses =
        (await vscode.commands.executeCommand<vscode.CodeLens[]>("vscode.executeCodeLensProvider", textFile)) ?? [];
      const runLenses = lenses
        .filter((l) => l.command?.command === "bynk.runTestCase")
        .map((l) => l.command?.arguments?.[0]);
      if (runLenses.length === names.length && names.every((n) => runLenses.includes(n))) return;
      if (Date.now() > deadline) throw new Error(`the tree never held ${names}; lenses: ${runLenses}`);
      await new Promise((r) => setTimeout(r, 400));
    }
  }

  /** The `bynk test` runs (not discovery) logged since `from`, as their arguments. */
  function runs(from: number): string[][] {
    return fake
      .read()
      .slice(from)
      .map((e: FakeLogEntry) => e.argv ?? [])
      .filter((a) => a[0] === "test" && !a.includes("--no-run"));
  }

  it("runs a file that holds a whole suite as one process, by --suite", async function () {
    this.timeout(90_000);
    const names = ["alpha", "beta", "gamma"];
    await discover(names);
    const from = fake.read().length;

    await vscode.commands.executeCommand("bynk.runTestsInFile", textFile);

    const ran = runs(from);
    assert.strictEqual(ran.length, 1, `one process, got ${JSON.stringify(ran)}`);
    const argv = ran[0];
    assert.deepStrictEqual(argv.slice(argv.indexOf("--suite")), ["--suite", "text"], "the suite, whole");
    assert.ok(!argv.includes("--case"), "every case of it is selected, so none is named");
    // Slice D: a run asks for the stream, so each suite reaches the tree as it ends.
    assert.ok(argv.includes("--format") && argv.includes("ndjson"));
  });

  it("runs several cases of a suite as one process, by --suite and each --case", async function () {
    this.timeout(90_000);
    const names = ["one a", "one b", "one c"];
    await discover(names, true, true);
    const from = fake.read().length;

    await vscode.commands.executeCommand("bynk.runTestsInFile", textFile);

    const ran = runs(from);
    assert.strictEqual(ran.length, 1, `one process, got ${JSON.stringify(ran)}`);
    const argv = ran[0];
    assert.deepStrictEqual(
      argv.slice(argv.indexOf("--suite")),
      ["--suite", "text", "--case", "one a", "--case", "one b", "--case", "one c"],
      "the suite, and only the file's cases of it",
    );
  });

  it("runs one process per case against a driver that cannot take a set, and remembers it", async function () {
    this.timeout(90_000);
    process.env.BYNK_FAKE_SETS = "unsupported";
    const names = ["old a", "old b", "old c"];
    await discover(names, true, true);
    const from = fake.read().length;

    await vscode.commands.executeCommand("bynk.runTestsInFile", textFile);
    await waitFor(() => runs(from).filter((a) => !a.includes("--suite")).length >= 3, "the per-case runs");

    const ran = runs(from);
    assert.ok(ran[0].includes("--suite"), `the set was tried first: ${JSON.stringify(ran[0])}`);
    const single = ran.slice(1).map((a) => a[a.indexOf("--case") + 1]);
    assert.deepStrictEqual(single, names, `then one --case process per case, got ${JSON.stringify(ran)}`);
    assert.ok(ran.slice(1).every((a) => !a.includes("--suite")));

    // And it remembers: the next run does not try the set again.
    const again = fake.read().length;
    await vscode.commands.executeCommand("bynk.runTestsInFile", textFile);
    await waitFor(() => runs(again).length >= 3, "the second run's per-case processes");
    assert.ok(
      runs(again).every((a) => !a.includes("--suite")),
      `no set was tried again, got ${JSON.stringify(runs(again))}`,
    );
  });

  it("runs the whole project once when the file holds every test in it", async function () {
    this.timeout(90_000);
    // Names no other scenario uses, so `discover` can only return once *this*
    // discovery has landed: the reconcile that adds them also prunes the previous
    // scenario's `other` suite, in the same pass. Reusing a name from the test
    // above let it return early, with that suite still in the tree.
    const names = ["delta", "epsilon", "zeta"];
    await discover(names, false);
    const from = fake.read().length;

    await vscode.commands.executeCommand("bynk.runTestsInFile", textFile);

    const ran = runs(from);
    assert.strictEqual(ran.length, 1, `nothing narrows the run, got ${JSON.stringify(ran)}`);
    assert.ok(!ran[0].includes("--case"));
  });

  it("still runs the project for Run All when nothing has been discovered", async function () {
    this.timeout(90_000);
    // A project whose discovery found no tests (or has not finished, or hit a
    // compile error) leaves the tree empty. Run All must still run the project, as
    // it did before the plan existed: the run reports the diagnostics or fills the
    // tree. Reaching an empty-tree handler is only possible because Run All
    // dispatches the controller itself.
    await discover(["tmp one"], false);
    const empty = path.join(dir, "doc-empty.json");
    fs.writeFileSync(empty, JSON.stringify({ passed: 0, failed: 0, suites: [] }));
    process.env.BYNK_FAKE_TEST_DOC = empty;
    await vscode.commands.executeCommand("testing.refreshTests");
    for (const deadline = Date.now() + 45_000; ; ) {
      const lenses =
        (await vscode.commands.executeCommand<vscode.CodeLens[]>("vscode.executeCodeLensProvider", textFile)) ?? [];
      if (!lenses.some((l) => l.command?.command === "bynk.runTestCase")) break;
      if (Date.now() > deadline) throw new Error("the tree never emptied");
      await new Promise((r) => setTimeout(r, 400));
    }
    const from = fake.read().length;

    await vscode.commands.executeCommand("testing.runAll");
    await waitFor(() => runs(from).length > 0, "a project run");

    const ran = runs(from);
    assert.strictEqual(ran.length, 1, `one project run, got ${JSON.stringify(ran)}`);
    assert.ok(!ran[0].includes("--case"));
  });

  it("runs a file of many cases as one process too, with no per-case limit", async function () {
    this.timeout(90_000);
    const names = Array.from({ length: 7 }, (_, i) => `case ${i}`);
    await discover(names, true, true);
    const from = fake.read().length;

    await vscode.commands.executeCommand("bynk.runTestsInFile", textFile);

    const ran = runs(from);
    assert.strictEqual(ran.length, 1, `one process, got ${JSON.stringify(ran)}`);
    const argv = ran[0];
    assert.deepStrictEqual(
      argv.slice(argv.indexOf("--suite")),
      ["--suite", "text", ...names.flatMap((n) => ["--case", n])],
    );
  });
});
