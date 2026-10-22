// Unit coverage for the Test Explorer's run plan and outcome mapping (Test
// Explorer depth track, Slice A): which `bynk test` processes a selection needs,
// which cases a run may report, how a `skipped` case is read, and what Debug and
// "Run Tests in This File" do with the tree. `src/testPlan.ts` imports no
// `vscode`, so this compiles and runs against it directly.

import * as assert from "assert";

import {
  PER_CASE_LIMIT,
  caseKey,
  casesInFile,
  debugTarget,
  isWanted,
  outcomeOf,
  planRun,
  retriesNote,
  runNotes,
  skippedLine,
  suiteFile,
  unreported,
  type PlanCase,
} from "../../src/testPlan";

const A = "bynk-test:unit:orders";
const B = "bynk-test:integration:orders";
const C = "bynk-test:unit:billing";

/** orders (unit): joins, empty; orders (integration): joins; billing: charge. A
 *  name shared across suites (`joins`) is the interesting case. */
const TREE: PlanCase[] = [
  { suiteId: A, name: "joins", file: "/p/tests/orders.bynk" },
  { suiteId: A, name: "empty", file: "/p/tests/orders.bynk" },
  { suiteId: B, name: "joins", file: "/p/tests/integration_orders.bynk" },
  { suiteId: C, name: "charge", file: "/p/tests/billing.bynk" },
];

function manyCases(n: number): PlanCase[] {
  return Array.from({ length: n }, (_, i) => ({ suiteId: A, name: `case ${i}`, file: "/p/t.bynk" }));
}

describe("outcomeOf", () => {
  it("maps the driver's outcomes", () => {
    assert.strictEqual(outcomeOf("pass"), "passed");
    assert.strictEqual(outcomeOf("fail"), "failed");
    assert.strictEqual(outcomeOf("skipped"), "skipped");
    assert.strictEqual(outcomeOf("discovered"), "discovered");
  });

  it("does not read an outcome it does not know as a pass", () => {
    assert.strictEqual(outcomeOf("flaky"), "unknown");
    assert.strictEqual(outcomeOf(""), "unknown");
  });
});

describe("run notes", () => {
  it("is silent for an ordinary in-process run", () => {
    assert.deepStrictEqual(runNotes({}), []);
    assert.deepStrictEqual(runNotes({ skipped: 0 }), []);
  });

  it("names the target a remote run used and counts what it skipped", () => {
    assert.deepStrictEqual(runNotes({ transport: "remote:preview", skipped: 2 }), [
      "ran the system suites against preview",
      "2 cases were skipped",
    ]);
    assert.deepStrictEqual(runNotes({ skipped: 1 }), ["1 case was skipped"]);
  });

  it("gives a skipped case its reason when the driver supplied one", () => {
    assert.strictEqual(
      skippedLine({ name: "paid", message: "uses `advance Clock`" }),
      "skipped: paid — uses `advance Clock`",
    );
    assert.strictEqual(skippedLine({ name: "paid" }), "skipped: paid");
  });

  it("notes a retried case and nothing for an unretried one", () => {
    assert.strictEqual(retriesNote({ name: "paid", retries: 1 }), "paid: retried 1 time");
    assert.strictEqual(retriesNote({ name: "paid", retries: 3 }), "paid: retried 3 times");
    assert.strictEqual(retriesNote({ name: "paid" }), undefined);
    assert.strictEqual(retriesNote({ name: "paid", retries: 0 }), undefined);
  });
});

describe("planRun", () => {
  it("runs the whole project once for 'run all'", () => {
    const plan = planRun(TREE, {});
    assert.deepStrictEqual(plan.steps, [{}]);
    assert.strictEqual(plan.narrowed, false);
    assert.strictEqual(plan.wanted.length, TREE.length);
  });

  it("runs one case with --case", () => {
    const plan = planRun(TREE, { include: [{ suiteId: C, name: "charge" }] });
    assert.deepStrictEqual(plan.steps, [{ caseName: "charge" }]);
    assert.deepStrictEqual(plan.wanted, [caseKey(C, "charge")]);
    assert.strictEqual(plan.narrowed, true);
  });

  it("runs a handful of cases as one process each, in tree order", () => {
    const plan = planRun(TREE, {
      include: [
        { suiteId: C, name: "charge" },
        { suiteId: A, name: "empty" },
      ],
    });
    assert.deepStrictEqual(plan.steps, [{ caseName: "empty" }, { caseName: "charge" }]);
    assert.deepStrictEqual(plan.wanted, [caseKey(A, "empty"), caseKey(C, "charge")]);
  });

  it("runs the whole project once for a suite, and reports only that suite", () => {
    const plan = planRun(TREE, { include: [{ suiteId: A }] });
    assert.deepStrictEqual(plan.steps, [{}]);
    assert.deepStrictEqual(plan.wanted, [caseKey(A, "joins"), caseKey(A, "empty")]);
    assert.strictEqual(plan.narrowed, true);
  });

  it("falls back to one filtered whole-project run past the per-case limit", () => {
    const all = manyCases(PER_CASE_LIMIT + 3);
    const some = planRun(all, { include: all.slice(0, PER_CASE_LIMIT).map((c) => ({ suiteId: c.suiteId, name: c.name })) });
    assert.strictEqual(some.steps.length, PER_CASE_LIMIT);
    const more = planRun(all, { include: all.slice(0, PER_CASE_LIMIT + 1).map((c) => ({ suiteId: c.suiteId, name: c.name })) });
    assert.deepStrictEqual(more.steps, [{}]);
    assert.strictEqual(more.wanted.length, PER_CASE_LIMIT + 1);
  });

  it("honours the exclude list, for a suite and for a case", () => {
    const noSuite = planRun(TREE, { exclude: [{ suiteId: A }] });
    assert.deepStrictEqual(noSuite.wanted, [caseKey(B, "joins"), caseKey(C, "charge")]);
    assert.deepStrictEqual(noSuite.steps, [{}], "run all minus a suite is one filtered run");
    assert.strictEqual(noSuite.narrowed, true);

    const noCase = planRun(TREE, { include: [{ suiteId: A }], exclude: [{ suiteId: A, name: "joins" }] });
    assert.deepStrictEqual(noCase.wanted, [caseKey(A, "empty")]);
  });

  it("selects nothing when everything requested is excluded", () => {
    const plan = planRun(TREE, { include: [{ suiteId: C, name: "charge" }], exclude: [{ suiteId: C }] });
    assert.deepStrictEqual(plan.steps, []);
    assert.deepStrictEqual(plan.wanted, []);
  });

  it("filters a name shared by two suites to the one that was requested", () => {
    const plan = planRun(TREE, { include: [{ suiteId: A, name: "joins" }] });
    assert.deepStrictEqual(plan.steps, [{ caseName: "joins" }]);
    assert.strictEqual(isWanted(plan, A, "joins"), true);
    assert.strictEqual(isWanted(plan, B, "joins"), false, "--case ran the integration one too; it is not reported");
  });

  it("makes one process per distinct name when both clashing cases are requested", () => {
    const plan = planRun(TREE, {
      include: [
        { suiteId: A, name: "joins" },
        { suiteId: B, name: "joins" },
      ],
    });
    assert.deepStrictEqual(plan.steps, [{ caseName: "joins" }]);
    assert.strictEqual(plan.wanted.length, 2);
  });

  it("keeps a requested case the tree no longer holds, so it can be reported missing", () => {
    const plan = planRun(TREE, { include: [{ suiteId: A, name: "gone" }] });
    assert.deepStrictEqual(plan.wanted, [caseKey(A, "gone")]);
    assert.strictEqual(plan.narrowed, true);
    assert.deepStrictEqual(plan.steps, [{ caseName: "gone" }]);
  });

  it("limits a coverage run to one process", () => {
    assert.deepStrictEqual(
      planRun(TREE, { include: [{ suiteId: C, name: "charge" }] }, { coverage: true }).steps,
      [{ caseName: "charge" }],
    );
    const several = planRun(
      TREE,
      { include: [{ suiteId: A, name: "empty" }, { suiteId: C, name: "charge" }] },
      { coverage: true },
    );
    assert.deepStrictEqual(several.steps, [{}], "two cases would be two coverage reports");
    assert.deepStrictEqual(planRun(TREE, {}, { coverage: true }).steps, [{}]);
  });

  it("treats an empty tree as having nothing to run", () => {
    assert.deepStrictEqual(planRun([], {}).steps, []);
  });
});

describe("unreported", () => {
  it("lists the wanted keys no result reached", () => {
    const plan = planRun(TREE, { include: [{ suiteId: A }] });
    const reported = new Set([caseKey(A, "joins")]);
    assert.deepStrictEqual(unreported(plan, reported), [caseKey(A, "empty")]);
  });
});

describe("debugTarget", () => {
  it("debugs one selected case by name", () => {
    assert.deepStrictEqual(debugTarget(TREE, { include: [{ suiteId: C, name: "charge" }] }), {
      caseName: "charge",
    });
  });

  it("debugs the whole project, with no notice, when nothing narrows the selection", () => {
    assert.deepStrictEqual(debugTarget(TREE, {}), {});
  });

  it("debugs the whole project and says so for a suite or several cases", () => {
    const suite = debugTarget(TREE, { include: [{ suiteId: A }] });
    assert.strictEqual(suite.caseName, undefined);
    assert.match(suite.notice ?? "", /single case/);
    const several = debugTarget(TREE, {
      include: [{ suiteId: A, name: "empty" }, { suiteId: C, name: "charge" }],
    });
    assert.strictEqual(several.caseName, undefined);
    assert.match(several.notice ?? "", /single case/);
  });

  it("reports an empty selection as nothing to debug", () => {
    const none = debugTarget(TREE, { include: [{ suiteId: C }], exclude: [{ suiteId: C }] });
    assert.strictEqual(none.nothing, true);
    assert.match(none.notice ?? "", /nothing is selected/);
  });
});

describe("files and suites", () => {
  it("lists the cases declared in a file", () => {
    assert.deepStrictEqual(casesInFile(TREE, "/p/tests/orders.bynk"), [
      { suiteId: A, name: "joins" },
      { suiteId: A, name: "empty" },
    ]);
    assert.deepStrictEqual(casesInFile(TREE, "/p/tests/none.bynk"), []);
  });

  it("gives a suite a file only when all its cases share one", () => {
    assert.strictEqual(suiteFile(["/p/a.bynk", "/p/a.bynk"]), "/p/a.bynk");
    assert.strictEqual(suiteFile(["/p/a.bynk", "/p/b.bynk"]), undefined);
    assert.strictEqual(suiteFile(["/p/a.bynk", undefined]), undefined);
    assert.strictEqual(suiteFile([undefined]), undefined);
    assert.strictEqual(suiteFile([]), undefined);
  });
});
