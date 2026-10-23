// Reproduce and pin a failed generative test by seed (Test Explorer depth track,
// Slice C), inside the extension host against the stand-in `bynk` (#2094): which
// arguments the extension runs for a pinned case, a re-run with a seed, a
// whole-project run, an unpinned case and a pin gone stale.
//
// The stand-in serves a document shaped by the driver's own (`test_json.rs`):
// generative cases carry `kind`, and a failed one carries `seed` and
// `counterexample`. The tree's items are driven by id, as the Testing view's menu
// does with the item it was opened on.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { applyDocument, type RunSink } from "../../src/testing";
import { caseKey, planRun, type JsonCase, type TestRun } from "../../src/testPlan";
import { FAKE_BYNK, useFakeBynk, waitFor, type FakeLogEntry } from "./fakeBynk";

const FIXTURES = path.resolve(__dirname, "../../../test/fixtures");

const SUITE = "bynk-test:unit:gen";
const FAILING = "never seventy three";

describe("Seeds for generative tests", () => {
  const workspace = path.resolve(FIXTURES, "workspace");
  const textFile = vscode.Uri.file(path.join(workspace, "src", "text.bynk"));
  let dir: string;
  let fake: Awaited<ReturnType<typeof useFakeBynk>>;

  /** A document whose generative cases are `props` (the last one fails, with a
   *  seed and a counterexample) and whose one example-based case has no `kind`. */
  function writeDoc(file: string, props: string[]): string {
    const cases: unknown[] = [{ name: "an example", outcome: "pass", location: { path: "src/text.bynk", line: 1, col: 1 } }];
    props.forEach((name, i) => {
      const failing = name === FAILING;
      cases.push({
        name,
        outcome: failing ? "fail" : "pass",
        ...(failing ? { message: "property failed after 74 cases (seed 0x5f3a)" } : {}),
        kind: "property",
        ...(failing ? { seed: "0x5f3a", counterexample: "p = 73" } : {}),
        location: { path: "src/text.bynk", line: i + 2, col: 1 },
      });
    });
    const p = path.join(dir, file);
    fs.writeFileSync(p, JSON.stringify({ passed: cases.length - 1, failed: 1, suites: [{ name: "gen", kind: "unit", cases }] }));
    return p;
  }

  async function discover(doc: string, expect: string[]): Promise<void> {
    process.env.BYNK_FAKE_TEST_DOC = doc;
    await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(textFile));
    await vscode.commands.executeCommand("testing.refreshTests");
    const deadline = Date.now() + 45_000;
    for (;;) {
      const lenses =
        (await vscode.commands.executeCommand<vscode.CodeLens[]>("vscode.executeCodeLensProvider", textFile)) ?? [];
      const names = lenses.filter((l) => l.command?.command === "bynk.runTestCase").map((l) => l.command?.arguments?.[0]);
      if (expect.every((n) => names.includes(n))) return;
      if (Date.now() > deadline) throw new Error(`the tree never held ${expect}; lenses: ${names}`);
      await new Promise((r) => setTimeout(r, 400));
    }
  }

  function runs(from: number): string[][] {
    return fake
      .read()
      .slice(from)
      .map((e: FakeLogEntry) => e.argv ?? [])
      .filter((a) => a[0] === "test" && !a.includes("--no-run"));
  }

  /** Run the failing case from its lens and wait for its process. */
  async function runFailing(): Promise<string[]> {
    const from = fake.read().length;
    await vscode.commands.executeCommand("bynk.runTestCase", FAILING);
    await waitFor(() => runs(from).length > 0, "the case's run");
    return runs(from)[0];
  }

  const id = `${SUITE}::${FAILING}`;

  before(async function () {
    this.timeout(90_000);
    dir = fs.mkdtempSync(path.join(os.tmpdir(), "bynk-seeds-"));
    fs.chmodSync(FAKE_BYNK, 0o755);
    fake = await useFakeBynk();
    await discover(writeDoc("doc-before.json", ["stays at least zero", FAILING]), ["stays at least zero", FAILING]);
  });

  after(async () => {
    await vscode.commands.executeCommand("bynk.unpinSeed", { id });
    delete process.env.BYNK_FAKE_TEST_DOC;
    await fake.restore();
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it("draws a fresh seed for a case that has no pin", async function () {
    this.timeout(60_000);
    const argv = await runFailing();
    assert.deepStrictEqual(argv.slice(argv.indexOf("--case")), ["--case", FAILING]);
    assert.ok(!argv.includes("--seed"));
  });

  it("re-runs the case with the seed its failure reported", async function () {
    this.timeout(60_000);
    await runFailing(); // the failure whose seed is remembered
    const from = fake.read().length;
    await vscode.commands.executeCommand("bynk.rerunWithSeed", { id });
    await waitFor(() => runs(from).length > 0, "the re-run");
    const argv = runs(from)[0];
    assert.deepStrictEqual(argv.slice(argv.indexOf("--case")), ["--case", FAILING, "--seed", "0x5f3a"]);
  });

  it("passes a pinned seed to a run of that one case, and to nothing else", async function () {
    this.timeout(90_000);
    await runFailing();
    await vscode.commands.executeCommand("bynk.pinSeed", { id });

    const pinned = await runFailing();
    assert.deepStrictEqual(pinned.slice(pinned.indexOf("--case")), ["--case", FAILING, "--seed", "0x5f3a"]);

    // A whole-project run draws a fresh seed: one root seed cannot honour several pins.
    const from = fake.read().length;
    await vscode.commands.executeCommand("testing.runAll");
    await waitFor(() => runs(from).length > 0, "a whole-project run");
    assert.ok(!runs(from)[0].includes("--seed"), `no seed for a whole run: ${runs(from)[0]}`);

    await vscode.commands.executeCommand("bynk.unpinSeed", { id });
    const unpinned = await runFailing();
    assert.ok(!unpinned.includes("--seed"), "an unpinned case draws a fresh seed again");
  });

  it("holds back a pin once the suite's property tests have changed", async function () {
    this.timeout(90_000);
    await runFailing();
    await vscode.commands.executeCommand("bynk.pinSeed", { id });
    const fresh = await runFailing();
    assert.ok(fresh.includes("--seed"), "the pin holds while the suite is as it was");

    // A property added before the failing one shifts its ordinal, so the seed would
    // draw a different stream: the pin is stale and is not passed.
    await discover(writeDoc("doc-after.json", ["a new first property", "stays at least zero", FAILING]), [
      "a new first property",
    ]);
    const stale = await runFailing();
    assert.ok(!stale.includes("--seed"), `a stale pin is not passed: ${stale}`);
  });

  it("is not moved by a change to an example-based case", async function () {
    this.timeout(90_000);
    // Back to the first list of properties, with a pin taken against it.
    await discover(writeDoc("doc-before2.json", ["stays at least zero", FAILING]), ["stays at least zero"]);
    await vscode.commands.executeCommand("bynk.unpinSeed", { id });
    await runFailing();
    await vscode.commands.executeCommand("bynk.pinSeed", { id });

    const cases: { name: string }[] = JSON.parse(fs.readFileSync(path.join(dir, "doc-before2.json"), "utf8")).suites[0].cases;
    assert.ok(cases.some((c) => c.name === "an example"));
    const doc = JSON.parse(fs.readFileSync(path.join(dir, "doc-before2.json"), "utf8"));
    doc.suites[0].cases.unshift({ name: "another example", outcome: "pass", location: { path: "src/text.bynk", line: 40, col: 1 } });
    const edited = path.join(dir, "doc-edited.json");
    fs.writeFileSync(edited, JSON.stringify(doc));
    await discover(edited, ["another example"]);

    const argv = await runFailing();
    assert.ok(argv.includes("--seed"), `an example-based case does not stale a pin: ${argv}`);
  });
});

describe("a failed generative case reports its seed and counterexample", () => {
  it("adds them to the failure, in addition to the unchanged text", () => {
    const ctrl = vscode.tests.createTestController("bynk-seeds-test", "Seeds test");
    try {
      const suite = ctrl.createTestItem(SUITE, "gen");
      const item = ctrl.createTestItem(caseKey(SUITE, FAILING), FAILING);
      suite.children.add(item);
      const items = new Map([[item.id, item]]);
      const messages: string[] = [];
      const sink: RunSink = {
        started: () => undefined,
        passed: () => undefined,
        failed: (_t, m) => {
          for (const one of Array.isArray(m) ? m : [m]) {
            messages.push(typeof one.message === "string" ? one.message : one.message.value);
          }
        },
        skipped: () => undefined,
        appendOutput: () => undefined,
      };
      const doc: TestRun = {
        passed: 0,
        failed: 1,
        suites: [
          {
            name: "gen",
            kind: "unit",
            cases: [
              {
                name: FAILING,
                outcome: "fail",
                message: "property failed after 74 cases (seed 0x5f3a)",
                kind: "property",
                seed: "0x5f3a",
                counterexample: "p = 73",
              },
            ],
          },
        ],
      };
      const plan = planRun([{ suiteId: SUITE, name: FAILING }], {});
      applyDocument(sink, items, plan, doc, vscode.Uri.file(os.tmpdir()));
      assert.strictEqual(messages[0], "property failed after 74 cases (seed 0x5f3a)");
      assert.ok(messages.some((m) => m === "Counterexample: p = 73"), `${messages}`);
      assert.ok(messages.some((m) => m.startsWith("Seed: 0x5f3a")), `${messages}`);
    } finally {
      ctrl.dispose();
    }
  });
});

describe("a failed case names the declaration behind it (#2272)", () => {
  /** Run `cases` through `applyDocument` and give back what each failure message says
   *  and where clicking it goes. */
  function messagesOf(cases: JsonCase[]) {
    const ctrl = vscode.tests.createTestController("bynk-declaration-test", "Declaration test");
    try {
      const suite = ctrl.createTestItem(SUITE, "gen");
      const item = ctrl.createTestItem(caseKey(SUITE, FAILING), FAILING);
      suite.children.add(item);
      const items = new Map([[item.id, item]]);
      const seen: { text: string; where: string | undefined }[] = [];
      const sink: RunSink = {
        started: () => undefined,
        passed: () => undefined,
        failed: (_t, m) => {
          for (const one of Array.isArray(m) ? m : [m]) {
            seen.push({
              text: typeof one.message === "string" ? one.message : one.message.value,
              where:
                one.location &&
                `${path.basename(one.location.uri.fsPath)}:${one.location.range.start.line + 1}:${one.location.range.start.character + 1}`,
            });
          }
        },
        skipped: () => undefined,
        appendOutput: () => undefined,
      };
      const doc: TestRun = { passed: 0, failed: 1, suites: [{ name: "gen", kind: "unit", cases }] };
      const plan = planRun([{ suiteId: SUITE, name: FAILING }], {});
      applyDocument(sink, items, plan, doc, vscode.Uri.file(os.tmpdir()));
      return seen;
    } finally {
      ctrl.dispose();
    }
  }

  const declared = { kind: "invariant", agent: "Wallet", name: "nonneg" };

  it("adds a line for each refused step, each opening its declaration", () => {
    const seen = messagesOf([
      {
        name: FAILING,
        outcome: "fail",
        message: "history property failed after 3 runs (seed 0x5f3a)",
        location: { path: "src/tests/wallet.test.bynk", line: 46, col: 14 },
        kind: "history",
        seed: "0x5f3a",
        counterexample: "[Spend(1), Spend(1)]",
        refusals: [
          { index: 0, call: "Spend", ...declared, location: { path: "src/demo/wallet.bynk", line: 9, col: 13 } },
          // A refusal the driver could not place opens the case instead.
          { index: 1, call: "Spend", ...declared },
        ],
      },
    ]);
    const lines = seen.map((s) => [s.text, s.where]);
    assert.deepStrictEqual(lines[0], ["history property failed after 3 runs (seed 0x5f3a)", "wallet.test.bynk:46:14"]);
    assert.deepStrictEqual(lines[1], ["Step 1 (Spend) was refused by invariant Wallet.nonneg", "wallet.bynk:9:13"]);
    assert.deepStrictEqual(lines[2], ["Step 2 (Spend) was refused by invariant Wallet.nonneg", "wallet.test.bynk:46:14"]);
    assert.ok(lines.some(([t]) => t === "Counterexample: [Spend(1), Spend(1)]"), JSON.stringify(lines));
  });

  it("names the violation that failed the case and opens its declaration", () => {
    const seen = messagesOf([
      {
        name: FAILING,
        outcome: "fail",
        message: "Error: InvariantViolation: Wallet.nonneg",
        violation: { ...declared, location: { path: "src/demo/wallet.bynk", line: 9, col: 13 } },
      },
    ]);
    assert.deepStrictEqual(
      seen.map((s) => [s.text, s.where]),
      [
        ["Error: InvariantViolation: Wallet.nonneg", undefined],
        ["Violated invariant Wallet.nonneg", "wallet.bynk:9:13"],
      ],
    );
  });

  it("adds nothing to a case that no declaration refused", () => {
    const seen = messagesOf([{ name: FAILING, outcome: "fail", message: "boom" }]);
    assert.deepStrictEqual(seen.map((s) => s.text), ["boom"]);
  });
});
