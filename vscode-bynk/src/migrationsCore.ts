// The dependency-free core of the State Migrations view (#2468): the `bynk
// migrate status --format json` document, the argument list for `bynk migrate
// sweep`, the typed-confirmation rule for production environments, and the
// sweep plan's markup. No `vscode` import, so the unit tests compile against it
// directly (see tsconfig.test.json); `migrationsView.ts` wires it to the editor.
//
// DECISION C: the data is the driver's offline status report, never Cloudflare.
// Per-environment instance counts need the account, so the editor leaves them
// to the sweep itself: `bynk migrate sweep` lists the instances and prints them
// before its own confirmation, in the terminal the editor opens.
//
// Like deploy (ADR 0510), the editor never passes `--yes`: the driver's prompt
// stays the one audited confirmation path. `sweepArgs` is the one place the
// argument list is built, so that rule is enforced, and tested, here.

import { escapeHtml } from "./cliCore";
import { commandLine, validContextName, validEnvironmentName } from "./deployPlan";

/** One valid `migrate from N` step. */
export interface MigrationStep {
  from: number;
  /** The oldest stored version the step covers (`coversFrom..=from`). */
  coversFrom: number;
}

/** One agent class's migration state, as `bynk migrate status` reports it. */
export interface ClassStatus {
  /** `<context>.<Agent>`, the schema lock's key. */
  class: string;
  version: number;
  floor: number;
  oldestReadable: number;
  steps: MigrationStep[];
  /** Stored versions a record cannot be read forward from (`NoMigrationPath`). */
  withoutPath: number[];
  /** The environments `bynk.deploy.lock` records the class's Worker in. */
  deployedTo: string[];
}

const num = (v: unknown, d = 0): number => (typeof v === "number" && Number.isFinite(v) ? v : d);
const str = (v: unknown, d = ""): string => (typeof v === "string" ? v : d);
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const rec = (v: unknown): Record<string, unknown> =>
  v && typeof v === "object" ? (v as Record<string, unknown>) : {};

/** Parse the status document. Lenient: unknown fields are ignored and a missing
 *  list reads as empty, so a driver that adds to the report does not break the
 *  view. Throws only when the text is not a JSON object with `classes`. */
export function parseMigrateStatus(text: string): ClassStatus[] {
  const o = rec(JSON.parse(text) as unknown);
  if (!Array.isArray(o.classes)) throw new Error("not a `bynk migrate status` report (no `classes`)");
  return o.classes.map((c): ClassStatus => {
    const r = rec(c);
    const version = num(r.version);
    return {
      class: str(r.class),
      version,
      floor: num(r.floor),
      oldestReadable: num(r.oldest_readable, version),
      steps: arr(r.steps).map((s) => {
        const x = rec(s);
        return { from: num(x.from), coversFrom: num(x.covers_from, num(x.from)) };
      }),
      withoutPath: arr(r.without_path).map((v) => num(v)),
      deployedTo: arr(r.deployed_to).map((v) => str(v)),
    };
  });
}

/** What a status run came to: a report to show, or the driver's refusal. */
export type StatusOutcome =
  | { kind: "status"; classes: ClassStatus[] }
  | { kind: "failed"; detail: string };

/** Classify a finished `bynk migrate status --format json` run. */
export function statusOutcome(result: { code: number | null; stdout: string; stderr: string }): StatusOutcome {
  if (result.code === 0) {
    try {
      return { kind: "status", classes: parseMigrateStatus(result.stdout) };
    } catch {
      /* fall through: report what the driver printed */
    }
  }
  return { kind: "failed", detail: (result.stderr || result.stdout).trim() };
}

/** The agent's own name: the class key after its context. */
export function agentName(cls: string): string {
  return cls.slice(cls.lastIndexOf(".") + 1);
}

/** The context a class belongs to (its key before the agent's name). */
export function contextOf(cls: string): string {
  const i = cls.lastIndexOf(".");
  return i < 0 ? cls : cls.slice(0, i);
}

/** The steps the floor has passed: every stored record is already past them, so
 *  the next build warns `bynk.agent.migrate_below_floor` and they can be deleted. */
export function retirableSteps(c: ClassStatus): MigrationStep[] {
  return c.steps.filter((s) => s.from < c.floor);
}

/** The steps still in use, i.e. covering a version at or above the floor. */
export function liveSteps(c: ClassStatus): MigrationStep[] {
  return c.steps.filter((s) => s.from >= c.floor);
}

/** One line for a class: its version and floor, and the steps still live. */
export function classSummary(c: ClassStatus): string {
  const live = liveSteps(c).length;
  const parts = [`v${c.version}, floor ${c.floor}`];
  if (live > 0) parts.push(`${live} live step${live === 1 ? "" : "s"}`);
  if (retirableSteps(c).length > 0) parts.push("retirable steps");
  if (c.withoutPath.length > 0) parts.push("versions without a path");
  return parts.join(" · ");
}

/** The sweep's scope, as the picker and the command presets state it. */
export interface SweepTarget {
  /** One context (dotted or worker-dir form); absent sweeps the project. */
  context?: string;
  /** Environments to sweep; empty sweeps every recorded one. */
  environments: string[];
}

/** The `bynk migrate sweep` argument list. Never `--yes`: the driver's prompt
 *  is the confirmation. `--batch` and `--url` are left to a terminal. */
export function sweepArgs(target: SweepTarget): string[] {
  // The pickers and presets validate first; this is the backstop, so no value
  // can read as a flag such as `--yes` whatever the caller passed.
  if (target.context !== undefined && target.context !== "" && !validContextName(target.context)) {
    throw new Error(`not a context name: ${JSON.stringify(target.context)}`);
  }
  for (const e of target.environments) {
    if (!validEnvironmentName(e)) throw new Error(`not an environment name: ${JSON.stringify(e)}`);
  }
  const args = ["migrate", "sweep"];
  if (target.context) args.push("--context", target.context);
  for (const e of target.environments) args.push("--env", e);
  return args;
}

/** The classes a sweep of `target` covers: a context's classes (its dotted name
 *  or Worker-directory form), else all. */
export function sweptClasses(classes: ClassStatus[], target: SweepTarget): ClassStatus[] {
  if (!target.context) return classes;
  const want = target.context.replace(/-/g, ".");
  return classes.filter((c) => contextOf(c.class) === want || contextOf(c.class).replace(/\./g, "-") === target.context);
}

/** The environments a sweep touches: the ones named, else every one the swept
 *  classes are recorded in. Sorted, without duplicates. */
export function sweptEnvironments(classes: ClassStatus[], target: SweepTarget): string[] {
  const names = new Set<string>();
  if (target.environments.length > 0) for (const e of target.environments) names.add(e);
  else for (const c of sweptClasses(classes, target)) for (const e of c.deployedTo) names.add(e);
  return [...names].sort();
}

/** Whether an environment reads as production. The manifest has no marker for a
 *  sweep to read, so this is by name: the unnamed `default` environment (what a
 *  plain `bynk deploy` targets) and `prod`, `production` and `live`. Everything
 *  else is treated as a non-production environment. */
export function isProductionEnvironment(name: string): boolean {
  return /^(default|prod|production|live)$/i.test(name);
}

/** The phrase the user types before a sweep that touches production: the name
 *  of the environment, or `all environments` when it covers several. `undefined`
 *  when no production environment is involved. */
export function typedConfirmation(environments: string[]): string | undefined {
  const production = environments.filter(isProductionEnvironment);
  if (production.length === 0) return undefined;
  return environments.length === 1 ? environments[0] : "all environments";
}

/** Whether the run could raise a class's floor: only when it covers every
 *  environment the class's Worker is recorded in (the driver also holds the
 *  floor for 30 minutes after a deploy, and says so when the sweep runs). */
export function floorOutlook(c: ClassStatus, environments: string[]): string {
  if (c.deployedTo.length === 0) return "not deployed: nothing to sweep";
  const missing = c.deployedTo.filter((e) => !environments.includes(e));
  if (missing.length > 0) {
    return `floor stays at ${c.floor}: not swept in ${missing.join(", ")}`;
  }
  return `floor may rise from ${c.floor} once every instance has migrated`;
}

export interface SweepPlanOptions {
  target: SweepTarget;
  /** The phrase the user will be asked to type, if any. */
  confirmation?: string;
}

/** The sweep plan's markup. Buttons carry `data-action`; every CLI-supplied
 *  string is escaped. */
export function renderSweepPlanHtml(classes: ClassStatus[], opts: SweepPlanOptions): string {
  const e = escapeHtml;
  const swept = sweptClasses(classes, opts.target);
  const envs = sweptEnvironments(classes, opts.target);
  const out: string[] = [];
  out.push(
    `<header class="env"><h1>Sweep plan</h1><p class="scope">` +
      (opts.target.context ? `Context <code>${e(opts.target.context)}</code> only` : "Whole project") +
      `, environment${envs.length === 1 ? "" : "s"} ` +
      (envs.length > 0 ? envs.map((n) => `<span class="env-name">${e(n)}</span>`).join(", ") : "none recorded") +
      `</p></header>`,
  );
  out.push(
    `<section><h2>Classes</h2><ul class="rows">${swept
      .map(
        (c) =>
          `<li><code>${e(c.class)}</code> ${e(classSummary(c))}<br><span class="origin">${e(floorOutlook(c, envs))}</span></li>`,
      )
      .join("")}</ul></section>`,
  );
  out.push(
    `<section class="warnings"><h2>What a sweep does</h2><p>Wakes every stored instance of these classes in the deployed Workers, so each migrates to its class's current version. It sets a temporary secret, <code>BYNK_MIGRATE_KEY</code>, on each Worker and deletes it afterwards; each set and delete creates a Worker version. It needs <code>CLOUDFLARE_API_TOKEN</code> and <code>CLOUDFLARE_ACCOUNT_ID</code> in the environment the editor was started from.</p><p>The instance counts come from Cloudflare, so <code>bynk</code> lists them in the terminal and asks you to confirm there, before it changes anything.</p></section>`,
  );
  if (opts.confirmation) {
    out.push(
      `<p class="incomplete">This sweep touches production. Before the terminal opens you will be asked to type <code>${e(opts.confirmation)}</code>.</p>`,
    );
  }
  out.push(
    `<footer class="actions"><button class="primary" data-action="sweep">Sweep…</button><button data-action="refresh">Refresh status</button></footer>`,
  );
  out.push(`<p class="note">Sweep runs <code>${e(commandLine(sweepArgs(opts.target)))}</code> in a terminal.</p>`);
  return out.join("\n");
}
