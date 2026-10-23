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
  coverageScope,
  debugTarget,
  declarationNotes,
  handlerForCall,
  isWanted,
  outcomeOf,
  planRun,
  retriesNote,
  runNotes,
  skippedLine,
  suiteFile,
  suiteNameOf,
  carriesTiers,
  systemOnly,
  tierOf,
  unreported,
  type PlanCase,
} from "../../src/testPlan";
import {
  fingerprintOf,
  pinDescription,
  pinState,
  seedFor,
  validSeed,
  type Pin,
} from "../../src/testSeeds";
import { DEV_TARGET, offersSkipUnsupported, scanTargets, targetChoices, targetDetail } from "../../src/testTargets";

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

  it("runs the project for a coverage case whose name another suite shares, since --case would run both", () => {
    // `joins` is in suites A and B: `--case joins` measures both, which is not one
    // case's report, whether one or both were selected (#2269).
    for (const include of [
      [{ suiteId: A, name: "joins" }],
      [{ suiteId: A, name: "joins" }, { suiteId: B, name: "joins" }],
    ]) {
      assert.deepStrictEqual(planRun(TREE, { include }, { coverage: true }).steps, [{}]);
    }
    // A name only one suite has is still one case.
    assert.deepStrictEqual(
      planRun(TREE, { include: [{ suiteId: A, name: "empty" }] }, { coverage: true }).steps,
      [{ caseName: "empty" }],
    );
  });

  it("still runs the project for 'run all' when nothing has been discovered yet", () => {
    // Discovery may still be in flight, or may have hit a compile error: the run
    // itself reports the diagnostics or fills the tree, so it must not be skipped.
    const plan = planRun([], {});
    assert.deepStrictEqual(plan.steps, [{}]);
    assert.deepStrictEqual(plan.wanted, []);
    assert.strictEqual(plan.narrowed, false);
  });

  it("has nothing to run when an explicit selection comes to nothing", () => {
    assert.deepStrictEqual(planRun([], { exclude: [{ suiteId: A }] }).steps, []);
  });
});

describe("unreported", () => {
  it("lists the wanted keys no result reached", () => {
    const plan = planRun(TREE, { include: [{ suiteId: A }] });
    const reported = new Set([caseKey(A, "joins")]);
    assert.deepStrictEqual(unreported(plan, reported), [caseKey(A, "empty")]);
  });
});

describe("planRun with a set filter (#2273)", () => {
  const S1 = "bynk-test:unit:s1";
  const S2 = "bynk-test:unit:s2";
  /** s1: a, b, c; s2: d, e. */
  const SETS: PlanCase[] = [
    { suiteId: S1, name: "a" },
    { suiteId: S1, name: "b" },
    { suiteId: S1, name: "c" },
    { suiteId: S2, name: "d" },
    { suiteId: S2, name: "e" },
  ];
  const plan = (include: { suiteId: string; name?: string }[], exclude: { suiteId: string; name?: string }[] = []) =>
    planRun(SETS, { include, exclude }, { sets: true });

  it("names a driver suite by the item id's last part, whatever the kind", () => {
    assert.strictEqual(suiteNameOf("bynk-test:unit:orders"), "orders");
    assert.strictEqual(suiteNameOf("bynk-test:integration:orders"), "orders");
  });

  it("runs several cases of one suite as one process: the suite and each case", () => {
    assert.deepStrictEqual(plan([{ suiteId: S1, name: "a" }, { suiteId: S1, name: "b" }]).steps, [
      { suites: ["s1"], cases: ["a", "b"] },
    ]);
  });

  it("runs cases of several suites as one process, over the suites they belong to", () => {
    assert.deepStrictEqual(plan([{ suiteId: S1, name: "a" }, { suiteId: S2, name: "d" }]).steps, [
      { suites: ["s1", "s2"], cases: ["a", "d"] },
    ]);
  });

  it("runs a whole suite by --suite alone, and several the same way", () => {
    assert.deepStrictEqual(plan([{ suiteId: S1 }]).steps, [{ suites: ["s1"] }]);
    assert.deepStrictEqual(plan([{ suiteId: S1 }, { suiteId: S2 }]).narrowed, false, "everything is the whole project");
    assert.deepStrictEqual(
      planRun(SETS, { include: [{ suiteId: S2 }] }, { sets: true }).steps,
      [{ suites: ["s2"] }],
    );
  });

  it("makes a suite partial by excluding one of its cases", () => {
    assert.deepStrictEqual(plan([{ suiteId: S1 }], [{ suiteId: S1, name: "c" }]).steps, [
      { suites: ["s1"], cases: ["a", "b"] },
    ]);
  });

  it("names no cases when a fully selected suite sits beside a partial one, so its other cases still run", () => {
    // `--suite s1 --suite s2 --case a` would run only `a` in s1. `--suite` alone
    // runs s2 whole, and only the wanted cases are reported.
    const mixed = plan([{ suiteId: S1 }, { suiteId: S2, name: "d" }]);
    assert.deepStrictEqual(mixed.steps, [{ suites: ["s1", "s2"] }]);
    assert.deepStrictEqual(mixed.wanted, [
      caseKey(S1, "a"),
      caseKey(S1, "b"),
      caseKey(S1, "c"),
      caseKey(S2, "d"),
    ]);
    assert.strictEqual(mixed.narrowed, true);
  });

  it("keeps one case as --case, as before", () => {
    assert.deepStrictEqual(plan([{ suiteId: S2, name: "e" }]).steps, [{ caseName: "e" }]);
  });

  it("treats a selection of everything as the whole project", () => {
    assert.deepStrictEqual(planRun(SETS, {}, { sets: true }).steps, [{}]);
  });

  it("reports a requested case the tree does not hold as missing, but keeps it out of the set", () => {
    // The driver fails a run for a `--suite` or `--case` that selects nothing, so a
    // stale item would fail the whole set. It stays in `wanted`, so it is still
    // reported missing.
    const stale = "bynk-test:unit:gone";
    const two = plan([{ suiteId: S1, name: "a" }, { suiteId: S1, name: "b" }, { suiteId: stale, name: "x" }]);
    assert.deepStrictEqual(two.steps, [{ suites: ["s1"], cases: ["a", "b"] }]);
    assert.ok(two.wanted.includes(caseKey(stale, "x")));
    const one = plan([{ suiteId: S1, name: "a" }, { suiteId: stale, name: "x" }]);
    assert.deepStrictEqual(one.steps, [{ caseName: "a" }], "one known case is --case");
    assert.ok(one.wanted.includes(caseKey(stale, "x")));
  });

  it("runs the project when everything requested is stale, so the results can say so", () => {
    const p = plan([{ suiteId: "bynk-test:unit:gone", name: "x" }, { suiteId: "bynk-test:unit:gone", name: "y" }]);
    assert.deepStrictEqual(p.steps, [{}]);
    assert.strictEqual(p.wanted.length, 2);
    assert.strictEqual(p.narrowed, true);
  });

  it("sends a run against a target the same set, instead of the whole project", () => {
    const include = [{ suiteId: S1, name: "a" }, { suiteId: S2, name: "d" }];
    assert.deepStrictEqual(planRun(SETS, { include }, { sets: true, remote: true }).steps, [
      { suites: ["s1", "s2"], cases: ["a", "d"] },
    ]);
    assert.deepStrictEqual(planRun(SETS, { include }, { remote: true }).steps, [{}], "without sets, as before");
  });

  it("leaves a coverage run as it was: several cases are one whole-project process", () => {
    const include = [{ suiteId: S1, name: "a" }, { suiteId: S1, name: "b" }];
    assert.deepStrictEqual(planRun(SETS, { include }, { sets: true, coverage: true }).steps, [{}]);
  });

  it("falls back to a process per case, and to the whole project, without sets", () => {
    const two = planRun(SETS, { include: [{ suiteId: S1, name: "a" }, { suiteId: S1, name: "b" }] });
    assert.deepStrictEqual(two.steps, [{ caseName: "a" }, { caseName: "b" }]);
    assert.deepStrictEqual(planRun(SETS, { include: [{ suiteId: S1 }] }).steps, [{}]);
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

  it("debugs the whole project, with no notice, when nothing has been discovered yet", () => {
    assert.deepStrictEqual(debugTarget([], {}), {});
  });

  it("warns when the one case's name is shared with another suite", () => {
    const shared = debugTarget(TREE, { include: [{ suiteId: A, name: "joins" }] });
    assert.strictEqual(shared.caseName, "joins");
    assert.match(shared.notice ?? "", /another suite has a test with the same name/);
    assert.deepStrictEqual(
      debugTarget(TREE, { include: [{ suiteId: A, name: "empty" }] }),
      { caseName: "empty" },
      "a name only one suite has needs no warning",
    );
  });

  it("debugs a suite or several cases as one set when the driver takes one", () => {
    const sets = { sets: true };
    assert.deepStrictEqual(debugTarget(TREE, { include: [{ suiteId: C }] }, sets), { caseName: "charge" });
    assert.deepStrictEqual(
      debugTarget(TREE, { include: [{ suiteId: A, name: "empty" }, { suiteId: C, name: "charge" }] }, sets),
      { suites: ["orders", "billing"] },
      "billing is whole beside a partial suite, so `--suite` alone",
    );
    // Both of a suite's cases is the whole suite, and nothing is left out, so no
    // notice is given.
    assert.deepStrictEqual(
      debugTarget(TREE, { include: [{ suiteId: A, name: "empty" }, { suiteId: A, name: "joins" }] }, sets),
      { suites: ["orders"] },
    );
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

describe("tierOf", () => {
  it("takes the driver's per-case tier", () => {
    assert.strictEqual(tierOf({ tier: "system" }, "unit"), "system");
    assert.strictEqual(tierOf({ tier: "integration" }, "unit"), "integration");
  });

  it("falls back to the suite kind only for a driver that predates the field", () => {
    assert.strictEqual(tierOf({}, "unit"), "unit");
    assert.strictEqual(tierOf({}, "integration"), "integration");
  });

  it("leaves a property, which has no tier, untagged under a driver that carries them", () => {
    assert.strictEqual(tierOf({}), undefined);
  });

  it("does not tag a tier it does not know", () => {
    assert.strictEqual(tierOf({ tier: "load" }, "weird"), undefined);
    assert.strictEqual(tierOf({}, "weird"), undefined);
  });

  it("reads a document as tier-carrying when any case has a tier", () => {
    assert.strictEqual(carriesTiers([{ cases: [{ name: "a", outcome: "pass" }] }]), false);
    assert.strictEqual(
      carriesTiers([{ cases: [{ name: "a", outcome: "pass" }, { name: "b", outcome: "pass", tier: "unit" }] }]),
      true,
    );
    assert.strictEqual(carriesTiers([]), false);
  });
});

describe("systemOnly", () => {
  const MIXED: PlanCase[] = [
    { suiteId: A, name: "joins", tier: "unit" },
    { suiteId: A, name: "survives", tier: "system" },
    { suiteId: B, name: "wired", tier: "system" },
    { suiteId: C, name: "charge" },
  ];

  it("keeps the system cases and drops the rest", () => {
    const r = systemOnly(MIXED, {});
    assert.deepStrictEqual(r.cases.map((c) => c.name), ["survives", "wired"]);
    assert.strictEqual(r.empty, false);
  });

  it("drops a named case that is not system, keeps a suite reference", () => {
    const r = systemOnly(MIXED, { include: [{ suiteId: A }, { suiteId: C, name: "charge" }] });
    assert.deepStrictEqual(r.request.include, [{ suiteId: A }]);
    const plan = planRun(r.cases, r.request, { remote: true });
    assert.deepStrictEqual(plan.wanted, [caseKey(A, "survives")]);
  });

  it("is empty when the selected suite holds no system case", () => {
    const r = systemOnly(MIXED, { include: [{ suiteId: C }] });
    assert.deepStrictEqual(r.request.include, []);
    assert.strictEqual(r.empty, true);
  });

  it("is empty when the selection holds no system case, or there is none", () => {
    assert.strictEqual(systemOnly(MIXED, { include: [{ suiteId: A, name: "joins" }] }).empty, true);
    assert.strictEqual(systemOnly([{ suiteId: A, name: "x", tier: "unit" }], {}).empty, true);
  });

  it("runs one process against a target, whatever the selection", () => {
    const cases = MIXED.filter((c) => c.tier === "system");
    const both = planRun(cases, { include: cases.map((c) => ({ suiteId: c.suiteId, name: c.name })) }, { remote: true });
    assert.deepStrictEqual(both.steps, [{}]);
    const one = planRun(cases, { include: [{ suiteId: B, name: "wired" }] }, { remote: true });
    assert.deepStrictEqual(one.steps, [{ caseName: "wired" }]);
  });
});

describe("scanTargets", () => {
  it("reads bare and quoted target names, in file order", () => {
    const toml = [
      "[package]",
      'name = "shop"',
      "",
      "[test.targets.preview]",
      'url = "https://preview.example.com"',
      "",
      '[test.targets."staging eu"]',
      "[test.targets.'qa']  # the shared one",
      "[ test . targets . nightly ]",
    ].join("\n");
    assert.deepStrictEqual(scanTargets(toml), ["preview", "staging eu", "qa", "nightly"]);
  });

  it("finds nothing in a manifest with no targets, or an empty one", () => {
    assert.deepStrictEqual(scanTargets(""), []);
    assert.deepStrictEqual(scanTargets("[package]\nname = \"x\"\n"), []);
  });

  it("does not read the lookalikes: a value, another table, an array of tables", () => {
    const toml = [
      'note = "[test.targets.fake]"',
      "[test.targetsx.nope]",
      "[[test.targets.listed]]",
      "[deploy.targets.prod]",
    ].join("\n");
    assert.deepStrictEqual(scanTargets(toml), []);
  });

  it("misses an inline or dotted form, which the driver still accepts when typed", () => {
    assert.deepStrictEqual(scanTargets('[test]\ntargets.x.url = "https://x.example.com"\n'), []);
  });

  it("lists a name once, and leaves `dev` to the built-in entry", () => {
    assert.deepStrictEqual(scanTargets("[test.targets.a]\n[test.targets.a]\n[test.targets.dev]\n"), ["a"]);
  });

  it("copes with CRLF line endings", () => {
    assert.deepStrictEqual(scanTargets("[test.targets.a]\r\nurl = \"x\"\r\n[test.targets.b]\r\n"), ["a", "b"]);
  });
});

describe("targetChoices", () => {
  it("offers dev first, then the manifest's targets", () => {
    assert.deepStrictEqual(targetChoices(["preview", "qa"]), [DEV_TARGET, "preview", "qa"]);
    assert.deepStrictEqual(targetChoices([]), [DEV_TARGET]);
  });

  it("puts the last choice first while it is still on offer", () => {
    assert.deepStrictEqual(targetChoices(["preview", "qa"], "qa"), ["qa", DEV_TARGET, "preview"]);
    assert.deepStrictEqual(targetChoices(["preview"], "gone"), [DEV_TARGET, "preview"]);
  });

  it("says a deployed target keeps its state, and dev does not", () => {
    assert.match(targetDetail("preview"), /keeps its state/);
    assert.doesNotMatch(targetDetail(DEV_TARGET), /keeps its state/);
  });
});

describe("offersSkipUnsupported", () => {
  it("recognises the driver's unsupported-case refusal", () => {
    assert.strictEqual(
      offersSkipUnsupported([
        "tests/a.bynk:3:1: error[T0412]: case uses `advance Clock`; run it in process, or pass `--skip-unsupported` to skip it with this reason",
      ]),
      true,
    );
  });

  it("does not offer it for any other compile failure", () => {
    assert.strictEqual(offersSkipUnsupported(["src/a.bynk:1:1: error[T0001]: unknown name"]), false);
    assert.strictEqual(offersSkipUnsupported([]), false);
  });
});

describe("seeds", () => {
  it("accepts only the 0x hex the runner reports", () => {
    assert.strictEqual(validSeed("0x5f3a"), true);
    assert.strictEqual(validSeed("0X5F3A"), true);
    for (const bad of ["5f3a", "0x", "0xzz", "0x123456789", "--coverage", "", undefined, 3]) {
      assert.strictEqual(validSeed(bad), false, String(bad));
    }
  });

  it("fingerprints a suite by its generative cases only, in order", () => {
    assert.deepStrictEqual(
      fingerprintOf([
        { name: "an example" },
        { name: "p one", kind: "property" },
        { name: "another example" },
        { name: "h one", kind: "history" },
      ]),
      ["p one", "h one"],
    );
    assert.deepStrictEqual(fingerprintOf([{ name: "only an example" }]), []);
  });

  const pin: Pin = { seed: "0x5f3a", fingerprint: ["a", "b"] };

  it("calls a pin fresh while the generative cases are the same list", () => {
    assert.strictEqual(pinState(pin, ["a", "b"]), "fresh");
  });

  it("calls a pin stale when a generative case is added, removed, renamed or reordered", () => {
    for (const now of [["x", "a", "b"], ["a"], ["a", "c"], ["b", "a"], []]) {
      assert.strictEqual(pinState(pin, now), "stale", now.join(","));
    }
  });

  it("describes a pin by its seed, and a stale one as stale", () => {
    assert.strictEqual(pinDescription(pin, "fresh"), "seed 0x5f3a");
    assert.strictEqual(pinDescription(pin, "stale"), "seed 0x5f3a (stale)");
  });

  const pins = new Map([["s::p", pin]]);
  const fp = (now: string[]) => () => now;

  it("passes a fresh pin to a single-case run", () => {
    assert.deepStrictEqual(seedFor("s::p", pins, fp(["a", "b"]), "p"), { seed: "0x5f3a" });
  });

  it("passes nothing to a run that is not exactly one case, or has no pin", () => {
    assert.deepStrictEqual(seedFor(undefined, pins, fp(["a", "b"]), "p"), {});
    assert.deepStrictEqual(seedFor("s::other", pins, fp(["a", "b"]), "other"), {});
  });

  it("holds a stale pin back and says why", () => {
    const r = seedFor("s::p", pins, fp(["x", "a", "b"]), "p");
    assert.strictEqual(r.seed, undefined);
    assert.match(r.note ?? "", /stale/);
  });

  it("never passes a pin that is not a seed", () => {
    const bad = new Map([["s::p", { seed: "--coverage", fingerprint: [] }]]);
    const r = seedFor("s::p", bad, fp([]), "p");
    assert.strictEqual(r.seed, undefined);
    assert.match(r.note ?? "", /not a seed/);
  });
});

describe("coverageScope (#2269 DECISIONS E and F)", () => {
  const scope = (include: { suiteId: string; name?: string }[]) =>
    coverageScope(planRun(TREE, { include }, { coverage: true }));

  it("is the case when one case runs alone", () => {
    assert.deepStrictEqual(scope([{ suiteId: C, name: "charge" }]), { kind: "case", name: "charge" });
  });

  it("is a suite that holds one case, as that case", () => {
    // Billing has a single case, so selecting the suite is selecting the case.
    assert.deepStrictEqual(scope([{ suiteId: C }]), { kind: "case", name: "charge" });
  });

  it("is the project, saying the selection was narrower, for a suite or several cases", () => {
    assert.deepStrictEqual(scope([{ suiteId: A }]), { kind: "project", narrowed: true });
    assert.deepStrictEqual(
      scope([{ suiteId: A, name: "empty" }, { suiteId: C, name: "charge" }]),
      { kind: "project", narrowed: true },
    );
  });

  it("is the project, with nothing to say, when everything was asked for", () => {
    assert.deepStrictEqual(scope([]), { kind: "project", narrowed: false });
  });

  it("is the project for a case that shares its name with another suite's", () => {
    assert.deepStrictEqual(scope([{ suiteId: A, name: "joins" }]), { kind: "project", narrowed: true });
  });
});

describe("declarationNotes (#2272)", () => {
  const nonneg = { kind: "invariant", agent: "Wallet", name: "nonneg", location: { path: "src/demo/wallet.bynk", line: 9, col: 13 } };

  it("says nothing for a case that no declaration refused", () => {
    assert.deepStrictEqual(declarationNotes({}), []);
    assert.deepStrictEqual(declarationNotes({ refusals: [] }), []);
  });

  it("names the declaration that failed the case, and opens it", () => {
    assert.deepStrictEqual(declarationNotes({ violation: nonneg }), [
      { text: "Violated invariant Wallet.nonneg", location: nonneg.location },
    ]);
  });

  it("tells an invariant and a transition of one name apart", () => {
    const same = { agent: "Meter", name: "same" };
    const texts = declarationNotes({
      refusals: [
        { index: 0, call: "Bump", kind: "invariant", ...same },
        { index: 1, call: "Bump", kind: "transition", ...same },
      ],
    }).map((n) => n.text);
    assert.deepStrictEqual(texts, [
      "Step 1 (Bump) was refused by invariant Meter.same",
      "Step 2 (Bump) was refused by transition Meter.same",
    ]);
  });

  it("lists the violation first, then the refused steps in order, each with its own location", () => {
    const transition = { kind: "transition", agent: "Ledger", name: "never_decreases", location: { path: "src/demo/ledger.bynk", line: 18, col: 14 } };
    const notes = declarationNotes({
      violation: nonneg,
      refusals: [
        { index: 2, call: "Spend", ...nonneg },
        { index: 4, call: "Debit", ...transition },
      ],
    });
    assert.deepStrictEqual(
      notes.map((n) => [n.text, n.location?.line]),
      [
        ["Violated invariant Wallet.nonneg", 9],
        ["Step 3 (Spend) was refused by invariant Wallet.nonneg", 9],
        ["Step 5 (Debit) was refused by transition Ledger.never_decreases", 18],
      ],
    );
  });

  it("keeps a declaration the driver wrote with a null location", () => {
    const notes = declarationNotes({ violation: { ...nonneg, location: null } });
    assert.deepStrictEqual(notes, [{ text: "Violated invariant Wallet.nonneg", location: null }]);
  });

  it("keeps a declaration the driver could not place, without a location", () => {
    const { location: _omitted, ...unplaced } = nonneg;
    assert.deepStrictEqual(declarationNotes({ violation: unplaced }), [
      { text: "Violated invariant Wallet.nonneg", location: undefined },
    ]);
  });
});

describe("handlerForCall (#2359)", () => {
  const pos = { line: 0, character: 0 };
  const range = { start: pos, end: pos };
  const handler = (owner: string, name: string) => ({
    owner,
    symbol: { name: `call ${name}`, kind: 5, range, selectionRange: range },
  });

  it("finds the handler whose name is the call with its first letter lower-cased", () => {
    const spend = handler("Wallet", "spend");
    assert.strictEqual(handlerForCall([handler("Other", "spend"), spend], "Wallet", "Spend"), spend);
  });

  it("finds a handler whose own name starts with a capital by the call as written", () => {
    const spend = handler("Wallet", "Spend");
    assert.strictEqual(handlerForCall([spend], "Wallet", "Spend"), spend);
  });

  it("chooses none when the call fits two handlers, or none", () => {
    assert.strictEqual(handlerForCall([handler("Wallet", "spend"), handler("Wallet", "Spend")], "Wallet", "Spend"), undefined);
    assert.strictEqual(handlerForCall([handler("Wallet", "deposit")], "Wallet", "Spend"), undefined);
  });
});

describe("declarationNotes with a placed handler (#2359)", () => {
  it("adds a line after the refusal that opens the handler", () => {
    const refusal = { index: 0, call: "Spend", kind: "invariant", agent: "Wallet", name: "nonneg", location: { path: "w.bynk", line: 9, col: 13 } };
    const at = { path: "w.bynk", line: 20, col: 8 };
    const notes = declarationNotes({ refusals: [refusal] }, () => ({ name: "spend", location: at }));
    assert.deepStrictEqual(
      notes.map((n) => [n.text, n.location?.line]),
      [
        ["Step 1 (Spend) was refused by invariant Wallet.nonneg", 9],
        ["Step 1 called Wallet.spend", 20],
      ],
    );
  });
});
