// Unit coverage for the State Migrations view's pure core (#2468): parsing the
// pinned `bynk migrate status --format json` fixture (the driver's own test
// holds the other end), the sweep argument list (never `--yes`), the typed
// confirmation for production, and the plan's markup.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import {
  classSummary,
  floorOutlook,
  isProductionEnvironment,
  liveSteps,
  parseMigrateStatus,
  renderSweepPlanHtml,
  retirableSteps,
  statusOutcome,
  sweepArgs,
  sweptClasses,
  sweptEnvironments,
  typedConfirmation,
} from "../../src/migrationsCore";

const FIXTURE = fs.readFileSync(path.resolve(__dirname, "../../../test/fixtures/migrate-status.json"), "utf8");

describe("parseMigrateStatus", () => {
  it("reads the pinned report", () => {
    const [cart, order] = parseMigrateStatus(FIXTURE);
    assert.deepStrictEqual(cart, {
      class: "shop.Cart",
      version: 4,
      floor: 1,
      oldestReadable: 4,
      steps: [{ from: 3, coversFrom: 3 }],
      withoutPath: [1, 2],
      deployedTo: ["default", "staging"],
    });
    assert.strictEqual(order.class, "shop.Order");
    assert.deepStrictEqual(order.steps, []);
    assert.deepStrictEqual(order.deployedTo, []);
  });

  it("ignores unknown fields and reads missing lists as empty", () => {
    const [c] = parseMigrateStatus('{"classes":[{"class":"a.B","version":2,"floor":1,"future":{"x":1}}]}');
    assert.deepStrictEqual(c.steps, []);
    assert.deepStrictEqual(c.withoutPath, []);
    assert.deepStrictEqual(c.deployedTo, []);
    assert.strictEqual(c.oldestReadable, 2);
  });

  it("refuses text that is not a status report", () => {
    assert.throws(() => parseMigrateStatus("{}"));
    assert.throws(() => parseMigrateStatus("not json"));
  });
});

describe("statusOutcome", () => {
  it("is the report on success and the driver's words otherwise", () => {
    assert.strictEqual(statusOutcome({ code: 0, stdout: FIXTURE, stderr: "" }).kind, "status");
    assert.deepStrictEqual(statusOutcome({ code: 1, stdout: "", stderr: "the project has errors\n" }), {
      kind: "failed",
      detail: "the project has errors",
    });
    // A zero exit with a document that is not a report is a failure too.
    assert.strictEqual(statusOutcome({ code: 0, stdout: "[]", stderr: "" }).kind, "failed");
  });
});

describe("steps and summaries", () => {
  const [cart] = parseMigrateStatus(FIXTURE);

  it("splits steps by the floor: a step is retirable once the floor passes it", () => {
    assert.deepStrictEqual(liveSteps(cart).map((s) => s.from), [3]);
    assert.deepStrictEqual(retirableSteps(cart), []);
    const raised = { ...cart, floor: 4 };
    assert.deepStrictEqual(retirableSteps(raised).map((s) => s.from), [3]);
    assert.deepStrictEqual(liveSteps(raised), []);
  });

  it("summarises a class on one line", () => {
    assert.strictEqual(classSummary(cart), "v4, floor 1 · 1 live step · versions without a path");
  });
});

describe("sweepArgs", () => {
  it("builds the sweep and never passes --yes", () => {
    assert.deepStrictEqual(sweepArgs({ environments: [] }), ["migrate", "sweep"]);
    assert.deepStrictEqual(sweepArgs({ context: "shop", environments: ["staging", "default"] }), [
      "migrate",
      "sweep",
      "--context",
      "shop",
      "--env",
      "staging",
      "--env",
      "default",
    ]);
    for (const a of [sweepArgs({ environments: [] }), sweepArgs({ context: "shop", environments: ["x"] })]) {
      assert.ok(!a.includes("--yes"));
    }
  });

  it("refuses a value that could read as a flag", () => {
    assert.throws(() => sweepArgs({ environments: ["--yes"] }));
    assert.throws(() => sweepArgs({ environments: ["a b"] }));
    assert.throws(() => sweepArgs({ context: "--yes", environments: [] }));
  });
});

describe("scope and production", () => {
  const classes = parseMigrateStatus(FIXTURE);

  it("selects a context's classes by either name form", () => {
    assert.strictEqual(sweptClasses(classes, { environments: [] }).length, 2);
    assert.strictEqual(sweptClasses(classes, { context: "shop", environments: [] }).length, 2);
    assert.strictEqual(sweptClasses(classes, { context: "other", environments: [] }).length, 0);
    const dashed = [{ ...classes[0], class: "acme.shop.Cart" }];
    assert.strictEqual(sweptClasses(dashed, { context: "acme-shop", environments: [] }).length, 1);
  });

  it("sweeps the named environments, else every recorded one", () => {
    assert.deepStrictEqual(sweptEnvironments(classes, { environments: [] }), ["default", "staging"]);
    assert.deepStrictEqual(sweptEnvironments(classes, { environments: ["staging"] }), ["staging"]);
  });

  it("treats default, prod, production and live as production", () => {
    for (const n of ["default", "prod", "Production", "LIVE"]) assert.ok(isProductionEnvironment(n), n);
    for (const n of ["staging", "preview", "dev", "production-eu"]) assert.ok(!isProductionEnvironment(n), n);
  });

  it("asks for a typed confirmation only when production is involved", () => {
    assert.strictEqual(typedConfirmation(["staging"]), undefined);
    assert.strictEqual(typedConfirmation(["default"]), "default");
    assert.strictEqual(typedConfirmation(["production"]), "production");
    assert.strictEqual(typedConfirmation(["default", "staging"]), "all environments");
    assert.strictEqual(typedConfirmation([]), undefined);
  });

  it("says when a narrower run holds the floor", () => {
    const [cart, order] = classes;
    assert.match(floorOutlook(cart, ["staging"]), /stays at 1: not swept in default/);
    assert.match(floorOutlook(cart, ["default", "staging"]), /may rise/);
    assert.match(floorOutlook(order, ["staging"]), /not deployed/);
  });
});

describe("renderSweepPlanHtml", () => {
  const classes = parseMigrateStatus(FIXTURE);

  it("names the environment, the command and the typed phrase, and offers Sweep", () => {
    const html = renderSweepPlanHtml(classes, { target: { environments: ["default"] }, confirmation: "default" });
    assert.match(html, /data-action="sweep"/);
    assert.match(html, /bynk migrate sweep --env default/);
    assert.match(html, /type <code>default<\/code>/);
    assert.ok(!html.includes("--yes"));
  });

  it("omits the production note for staging and escapes driver strings", () => {
    const evil = [{ ...classes[0], class: "<img src=x>.Cart", deployedTo: ["staging"] }];
    const html = renderSweepPlanHtml(evil, { target: { environments: ["staging"] } });
    assert.ok(!html.includes("touches production"));
    assert.ok(!html.includes("<img"));
    assert.match(html, /&lt;img/);
  });
});
