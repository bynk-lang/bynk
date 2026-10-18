// Unit coverage for plan-first deploy's pure core (#2101): parsing the pinned
// `bynk deploy --dry-run --format json` fixture, rendering the plan view, the
// argument lists (never `--yes`, never `--prune`, no secret values), and the
// environments read from `bynk.deploy.lock`.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import {
  commandLine,
  deployArgs,
  lockEnvironments,
  parseDeployPlan,
  planOutcome,
  renderPlanHtml,
  settledState,
  validContextName,
  validEnvironmentName,
} from "../../src/deployPlan";

// The plan's shape follows `bynk/src/deploy/plan.rs` (`Plan`, `ContextPlan`):
// a multi-context staging plan with every kind of row and orphan.
const FIXTURE = fs.readFileSync(path.resolve(__dirname, "../../../test/fixtures/deploy-plan.json"), "utf8");

describe("parseDeployPlan", () => {
  it("reads the pinned plan", () => {
    const plan = parseDeployPlan(FIXTURE);
    assert.strictEqual(plan.environment, "staging");
    assert.deepStrictEqual(plan.order, ["commerce-payment-staging", "commerce-orders-staging"]);
    assert.strictEqual(plan.contexts.length, 2);
    const [payment, orders] = plan.contexts;
    assert.deepStrictEqual(payment.kv, [{ binding: "KV", action: "create", namespace: "commerce-payment-staging" }]);
    assert.deepStrictEqual(payment.queues, [{ action: "reuse", queue: "payment-intake-staging" }]);
    assert.deepStrictEqual(payment.durableObjects, [{ class: "Ledger", storage: "sqlite", reconciledBy: "Cloudflare" }]);
    assert.deepStrictEqual(
      payment.secrets.map((s) => `${s.origin}:${s.name}`),
      ["managed:BYNK_CALL_FROM_COMMERCE__ORDERS", "supplied:STRIPE_KEY", "declared:WEBHOOK_SECRET"],
    );
    assert.deepStrictEqual(payment.foreignServices, [{ binding: "SERVICE_BILLING", worker: "legacy-billing" }]);
    assert.strictEqual(payment.secretsComplete, true);
    assert.strictEqual(orders.action, "redeploy");
    assert.strictEqual(orders.secretsComplete, false);
    assert.deepStrictEqual(orders.bindsTo, ["commerce-payment-staging"]);
    assert.deepStrictEqual(plan.orphans, {
      kv: [{ worker: "legacy-staging" }, { worker: "commerce-orders-staging", binding: "KV_CARTS" }],
      workers: ["legacy-staging"],
      queues: ["old-intake-staging"],
    });
  });

  it("ignores unknown fields and reads missing lists as empty", () => {
    const plan = parseDeployPlan(
      '{"environment":"default","future":{"x":1},"contexts":[{"worker":"api","action":"deploy","new_field":[1]}]}',
    );
    assert.deepStrictEqual(plan.order, []);
    assert.deepStrictEqual(plan.orphans, { kv: [], workers: [], queues: [] });
    assert.deepStrictEqual(plan.contexts[0].secrets, []);
    assert.strictEqual(plan.contexts[0].secretsComplete, true);
  });

  it("refuses text that is not a plan", () => {
    assert.throws(() => parseDeployPlan('{"environment":"default"}'));
    assert.throws(() => parseDeployPlan("bynk: refusing to deploy"));
  });
});

describe("deployArgs", () => {
  it("plans with --dry-run --format json, and deploys with the same target", () => {
    const target = { environment: "staging", context: "commerce.orders", secretsFile: "/tmp/s.env" };
    assert.deepStrictEqual(deployArgs(target, "plan"), [
      "deploy", "--dry-run", "--format", "json", "--env", "staging", "--context", "commerce.orders", "--secrets-file", "/tmp/s.env",
    ]);
    assert.deepStrictEqual(deployArgs(target, "deploy"), [
      "deploy", "--env", "staging", "--context", "commerce.orders", "--secrets-file", "/tmp/s.env",
    ]);
    assert.deepStrictEqual(deployArgs({ environment: "default" }, "deploy"), ["deploy", "--env", "default"]);
  });

  it("refuses a flag-like environment or context outright", () => {
    assert.throws(() => deployArgs({ environment: "--yes" }, "deploy"));
    assert.throws(() => deployArgs({ environment: "prod", context: "--yes" }, "deploy"));
    assert.throws(() => deployArgs({ environment: "prod", context: "--prune" }, "plan"));
  });

  it("never passes --yes, --prune, --force or a secret value", () => {
    for (const mode of ["plan", "deploy"] as const) {
      const args = deployArgs({ environment: "prod", context: "a", secretsFile: "s.env" }, mode);
      for (const banned of ["--yes", "--prune", "--force", "--secret"]) {
        assert.ok(!args.includes(banned), `${mode}: ${banned}`);
      }
    }
  });
});

describe("renderPlanHtml", () => {
  const plan = parseDeployPlan(FIXTURE);

  it("names the environment, the order, and every context's rows", () => {
    const html = renderPlanHtml(plan, { target: { environment: "staging" }, canDeploy: true });
    assert.ok(html.includes('Deploy plan: <span class="env-name">staging</span>'), html);
    assert.ok(/<ol class="order"><li><code>commerce-payment-staging<\/code><\/li><li><code>commerce-orders-staging<\/code>/.test(html));
    assert.ok(html.includes("KV <code>KV</code> → <code>commerce-payment-staging</code>"));
    assert.ok(html.includes("queue <code>payment-intake-staging</code>"));
    assert.ok(html.includes("Durable Object <code>Ledger</code>"));
    assert.ok(html.includes("foreign Worker <code>legacy-billing</code>"));
    assert.ok(html.includes("calls <code>commerce-payment-staging</code>"));
  });

  it("highlights required secrets and an incomplete secret list", () => {
    const html = renderPlanHtml(plan, { target: { environment: "staging" }, canDeploy: false });
    assert.ok(/<li class="required">.*WEBHOOK_SECRET/.test(html), html);
    assert.ok(!/<li class="required">.*STRIPE_KEY/.test(html));
    assert.ok(html.includes("cannot name"));
  });

  it("shows orphans with a prune-from-terminal hint, never a prune button", () => {
    const html = renderPlanHtml(plan, { target: { environment: "staging" }, canDeploy: true });
    assert.ok(html.includes("<code>bynk deploy --env staging --prune</code> in a terminal"));
    assert.ok(html.includes("Queue <code>old-intake-staging</code>"));
    assert.ok(html.includes("KV namespace <code>KV_CARTS</code> of <code>commerce-orders-staging</code>"));
    assert.ok(!/data-action="prune"/.test(html));
  });

  it("offers Deploy only from Bynk: Deploy…", () => {
    const deploy = renderPlanHtml(plan, { target: { environment: "staging" }, canDeploy: true });
    assert.ok(deploy.includes('data-action="deploy"'));
    assert.ok(deploy.includes("<code>bynk deploy --env staging</code> in a terminal"));
    const preview = renderPlanHtml(plan, { target: { environment: "staging" }, canDeploy: false });
    assert.ok(!preview.includes('data-action="deploy"'));
  });

  it("escapes CLI-supplied text", () => {
    const evil = parseDeployPlan(
      '{"environment":"<img/src=x>","order":[],"contexts":[{"worker":"<script>","action":"deploy"}]}',
    );
    const html = renderPlanHtml(evil, { target: { environment: "<img/src=x>" }, canDeploy: true, warnings: "<b>warn</b>" });
    // Every `<` from the input arrives escaped: each injected tag appears only
    // in its escaped form, never raw (whatever its case).
    for (const [tag, escaped] of [["<script>", "&lt;script&gt;"], ["<img/src=x>", "&lt;img/src=x&gt;"], ["<b>", "&lt;b&gt;"]]) {
      assert.ok(html.includes(escaped), `${tag} is escaped`);
      assert.ok(!html.toLowerCase().includes(tag), `${tag} does not appear raw`);
    }
    assert.ok(html.includes("&lt;script&gt;"));
  });
});

describe("lockEnvironments and validEnvironmentName", () => {
  it("lists the lock's environment tables", () => {
    const lock = [
      "version = 1",
      "[environments.default]",
      "workers = [\"api\"]",
      "[environments.default.kv.api]",
      "id = \"abc\"",
      "[environments.staging.kv.api]",
      "[environments.\"pr-12\"]",
    ].join("\n");
    assert.deepStrictEqual(lockEnvironments(lock), ["default", "pr-12", "staging"]);
    assert.deepStrictEqual(lockEnvironments(""), []);
  });

  it("accepts the names `--env` takes, refusing only what misreads as a flag", () => {
    // The driver takes any string for `--env`; a lock can record one TOML
    // must quote, and the picker must still offer it.
    for (const ok of ["staging", "pr-12_b", "Staging.v2", "eu/west", "1"]) {
      assert.ok(validEnvironmentName(ok), ok);
    }
    for (const bad of ["", "a b", "--yes", "-x", "tab\tname"]) {
      assert.ok(!validEnvironmentName(bad), bad);
    }
    assert.deepStrictEqual(lockEnvironments('[environments."Staging.v2"]\n[environments."-bad"]'), ["Staging.v2"]);
  });

  it("accepts context names in dotted or worker-dir form, never a flag", () => {
    for (const ok of ["orders", "commerce.orders", "commerce-orders", "a_b.c_d"]) {
      assert.ok(validContextName(ok), ok);
    }
    for (const bad of ["", "--yes", "-orders", "commerce..orders", "a b", ".orders", "orders."]) {
      assert.ok(!validContextName(bad), bad);
    }
  });
});

describe("commandLine", () => {
  it("quotes arguments a shell would split", () => {
    assert.strictEqual(commandLine(["deploy", "--secrets-file", "/a b/s.env"]), "bynk deploy --secrets-file '/a b/s.env'");
  });
});

describe("planOutcome and settledState", () => {
  const ok = { code: 0, stdout: FIXTURE, stderr: "warning: unpinned caller" };

  it("shows a plan, with the dry run's warnings", () => {
    const o = planOutcome(ok, false);
    assert.strictEqual(o.kind, "plan");
    if (o.kind === "plan") assert.strictEqual(o.warnings, "warning: unpinned caller");
  });

  it("drops a cancelled dry run silently, whatever it printed", () => {
    assert.deepStrictEqual(planOutcome({ code: null, stdout: "", stderr: "" }, true), { kind: "cancelled" });
    assert.deepStrictEqual(planOutcome(ok, true), { kind: "cancelled" });
  });

  it("reports a refusal with the driver's own text", () => {
    assert.deepStrictEqual(planOutcome({ code: 1, stdout: "", stderr: "bynk: contract skew" }, false), {
      kind: "failed",
      detail: "bynk: contract skew",
    });
    assert.strictEqual(planOutcome({ code: 0, stdout: "not json", stderr: "" }, false).kind, "failed");
  });

  it("keeps the previous target when a refresh after Choose secrets file… fails", () => {
    const shown = { target: { environment: "staging" } };
    const withSecrets = { target: { environment: "staging", secretsFile: "/tmp/bad.env" } };
    const failed = planOutcome({ code: 1, stdout: "", stderr: "bynk: cannot read /tmp/bad.env" }, false);
    assert.strictEqual(settledState(shown, withSecrets, failed), shown);
    assert.strictEqual(settledState(shown, withSecrets, planOutcome(ok, true)), shown);
    assert.strictEqual(settledState(shown, withSecrets, planOutcome(ok, false)), withSecrets);
    assert.strictEqual(settledState(undefined, withSecrets, failed), undefined);
  });
});
