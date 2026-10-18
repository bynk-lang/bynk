// The dependency-free core of plan-first deploy (#2101): the `bynk deploy
// --dry-run --format json` plan, the argument lists the editor runs, the
// environments recorded in `bynk.deploy.lock`, and the plan view's markup. No
// `vscode` import, so the unit tests compile against it directly (see
// tsconfig.test.json); `deploy.ts` wires it to the editor.
//
// The editor never passes `--yes` (DECISION A: the driver's own prompt is the
// final gate) or `--prune` (DECISION B), and never handles a secret value
// (DECISION C: only a `--secrets-file` path). `deployArgs` is the one place the
// argument list is built, so those three rules are enforced, and tested, here.

import { escapeHtml } from "./cliCore";

export interface PlanKv {
  binding: string;
  action: string;
  namespace: string;
}
export interface PlanQueue {
  action: string;
  queue: string;
}
export interface PlanDurableObject {
  class: string;
  storage: string;
  reconciledBy: string;
}
/** `declared` (required: an actor's auth secret), `read` (read, but optional),
 *  `supplied` (named by the user), `managed` (an edge key `bynk` mints). */
export interface PlanSecret {
  name: string;
  origin: string;
  action: string;
}
export interface PlanForeignService {
  binding: string;
  worker: string;
}
export interface ContextPlan {
  worker: string;
  /** `deploy` the first time, `redeploy` after. */
  action: string;
  kv: PlanKv[];
  queues: PlanQueue[];
  durableObjects: PlanDurableObject[];
  secrets: PlanSecret[];
  /** False when the context reads secrets the compiler cannot name, so
   *  `secrets` is not everything it needs. */
  secretsComplete: boolean;
  bindsTo: string[];
  foreignServices: PlanForeignService[];
}
export interface PlanOrphans {
  kv: { worker: string; binding?: string }[];
  workers: string[];
  queues: string[];
}
export interface DeployPlan {
  environment: string;
  /** Upload order, dependencies first. */
  order: string[];
  contexts: ContextPlan[];
  orphans: PlanOrphans;
}

const str = (v: unknown, d = ""): string => (typeof v === "string" ? v : d);
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const rec = (v: unknown): Record<string, unknown> =>
  v && typeof v === "object" ? (v as Record<string, unknown>) : {};

/** Parse the dry-run plan. Lenient: unknown fields are ignored and a missing
 *  list reads as empty, so a driver that adds to the plan does not break the
 *  view. Throws only when the text is not a JSON object with `contexts`. */
export function parseDeployPlan(text: string): DeployPlan {
  const raw = JSON.parse(text) as unknown;
  const o = rec(raw);
  if (!Array.isArray(o.contexts)) throw new Error("not a `bynk deploy` plan (no `contexts`)");
  const orphans = rec(o.orphans);
  return {
    environment: str(o.environment, "default"),
    order: arr(o.order).map((w) => str(w)),
    orphans: {
      kv: arr(orphans.kv).map((k) => {
        const r = rec(k);
        return typeof r.binding === "string"
          ? { worker: str(r.worker), binding: r.binding }
          : { worker: str(r.worker) };
      }),
      workers: arr(orphans.workers).map((w) => str(w)),
      queues: arr(orphans.queues).map((q) => str(q)),
    },
    contexts: o.contexts.map((c): ContextPlan => {
      const r = rec(c);
      return {
        worker: str(r.worker),
        action: str(r.action, "deploy"),
        kv: arr(r.kv).map((k) => {
          const x = rec(k);
          return { binding: str(x.binding), action: str(x.action), namespace: str(x.namespace) };
        }),
        queues: arr(r.queues).map((q) => {
          const x = rec(q);
          return { action: str(x.action), queue: str(x.queue) };
        }),
        durableObjects: arr(r.durable_objects).map((d) => {
          const x = rec(d);
          return { class: str(x.class), storage: str(x.storage), reconciledBy: str(x.reconciled_by) };
        }),
        secrets: arr(r.secrets).map((s) => {
          const x = rec(s);
          return { name: str(x.name), origin: str(x.origin), action: str(x.action) };
        }),
        secretsComplete: r.secrets_complete !== false,
        bindsTo: arr(r.binds_to).map((w) => str(w)),
        foreignServices: arr(r.foreign_services).map((f) => {
          const x = rec(f);
          return { binding: str(x.binding), worker: str(x.worker) };
        }),
      };
    }),
  };
}

/** What a deploy targets. */
export interface DeployTarget {
  environment: string;
  /** One context (dotted or worker-dir form); absent deploys the project. */
  context?: string;
  /** A dotenv-style `NAME=value` file. Only its path is handled. */
  secretsFile?: string;
}

/** The `bynk deploy` argument list: the dry-run plan, or the real deploy. The
 *  two differ only in `--dry-run --format json`, so the deploy runs exactly
 *  what was previewed. Never `--yes`, never `--prune`. */
export function deployArgs(target: DeployTarget, mode: "plan" | "deploy"): string[] {
  // The pickers and presets validate first; this is the backstop, so no value
  // can ever read as a flag such as `--yes` whatever the caller passed.
  if (!validEnvironmentName(target.environment)) {
    throw new Error(`not an environment name: ${JSON.stringify(target.environment)}`);
  }
  if (target.context !== undefined && !validContextName(target.context)) {
    throw new Error(`not a context name: ${JSON.stringify(target.context)}`);
  }
  const args = ["deploy"];
  if (mode === "plan") args.push("--dry-run", "--format", "json");
  args.push("--env", target.environment);
  if (target.context) args.push("--context", target.context);
  if (target.secretsFile) args.push("--secrets-file", target.secretsFile);
  return args;
}

/** The environments a `bynk.deploy.lock` records (its `[environments.<name>]`
 *  tables), sorted, without duplicates. */
export function lockEnvironments(lockText: string): string[] {
  const names = new Set<string>();
  for (const m of lockText.matchAll(/^\s*\[\[?\s*environments\.(?:"([^"]+)"|'([^']+)'|([A-Za-z0-9_-]+))/gm)) {
    names.add(m[1] ?? m[2] ?? m[3]);
  }
  return [...names].filter(validEnvironmentName).sort();
}

/** Whether `name` is a usable environment name. The driver takes any string
 *  for `--env` (it only suffixes resource names with it), so this refuses just
 *  what would misread on the command line: an empty name, whitespace, and a
 *  leading `-`, which would read as a flag (`--env --yes`). */
export function validEnvironmentName(name: string): boolean {
  return /^[^\s-]\S*$/.test(name);
}

/** Whether `name` is a usable `--context` value: a dotted context name
 *  (`commerce.orders`) or its worker-dir form (`commerce-orders`). Never
 *  flag-like, so a preset can't put `--yes` after `--context`. */
export function validContextName(name: string): boolean {
  return /^[A-Za-z_][A-Za-z0-9_]*(?:[.-][A-Za-z0-9_]+)*$/.test(name);
}

/** The shell-free command line a reader would type, for display only. */
export function commandLine(args: string[]): string {
  return ["bynk", ...args.map((a) => (/^[\w./:=@-]+$/.test(a) ? a : `'${a.replace(/'/g, `'\\''`)}'`))].join(" ");
}

/** What a dry run came to: a plan to show, a refusal to report, or a
 *  cancellation to drop silently (the user pressed Cancel). */
export type PlanOutcome =
  | { kind: "plan"; plan: DeployPlan; warnings: string }
  | { kind: "failed"; detail: string }
  | { kind: "cancelled" };

/** Classify a finished `bynk deploy --dry-run --format json` run. */
export function planOutcome(
  result: { code: number | null; stdout: string; stderr: string },
  cancelled: boolean,
): PlanOutcome {
  if (cancelled) return { kind: "cancelled" };
  if (result.code === 0) {
    try {
      return { kind: "plan", plan: parseDeployPlan(result.stdout), warnings: result.stderr };
    } catch {
      /* fall through: report what the driver printed */
    }
  }
  return { kind: "failed", detail: result.stderr || result.stdout };
}

/** The view's state after an attempt: the attempted target only when it
 *  produced a plan. A failed or cancelled refresh (say, after "Choose secrets
 *  file…") keeps the previous target and plan, so the view never shows a
 *  plan for a target it didn't plan. */
export function settledState<S>(previous: S | undefined, attempted: S, outcome: PlanOutcome): S | undefined {
  return outcome.kind === "plan" ? attempted : previous;
}

export interface PlanViewOptions {
  target: DeployTarget;
  /** Offer the Deploy button (Bynk: Deploy…); `false` for Show Deploy Plan. */
  canDeploy: boolean;
  /** Warnings the dry run printed on stderr, shown above the plan. */
  warnings?: string;
}

const ORIGIN_NOTE: Record<string, string> = {
  declared: "required: give it a value with a secrets file, the environment, or the driver's prompt",
  read: "read by a handler; optional",
  supplied: "named by you",
  managed: "minted by bynk",
};

/** The plan view's body markup. Buttons carry `data-action`, which the host
 *  view's script posts back. Every CLI-supplied string is escaped. */
export function renderPlanHtml(plan: DeployPlan, opts: PlanViewOptions): string {
  const e = escapeHtml;
  const env = e(plan.environment);
  const out: string[] = [];
  out.push(`<header class="env"><h1>Deploy plan: <span class="env-name">${env}</span></h1>`);
  out.push(
    `<p class="scope">${opts.target.context ? `Context <code>${e(opts.target.context)}</code> only` : "Whole project"}` +
      (opts.target.secretsFile ? `, secrets from <code>${e(opts.target.secretsFile)}</code>` : "") +
      `</p></header>`,
  );
  if (opts.warnings && opts.warnings.trim()) {
    out.push(`<section class="warnings"><h2>Warnings</h2><pre>${e(opts.warnings.trim())}</pre></section>`);
  }
  if (plan.order.length > 0) {
    out.push(
      `<section><h2>Order</h2><ol class="order">${plan.order.map((w) => `<li><code>${e(w)}</code></li>`).join("")}</ol></section>`,
    );
  }
  const orphanCount = plan.orphans.kv.length + plan.orphans.workers.length + plan.orphans.queues.length;
  if (orphanCount > 0) {
    const items = [
      ...plan.orphans.workers.map((w) => `<li>Worker <code>${e(w)}</code></li>`),
      ...plan.orphans.kv.map(
        (k) => `<li>KV namespace <code>${e(k.binding ?? "KV")}</code> of <code>${e(k.worker)}</code></li>`,
      ),
      ...plan.orphans.queues.map((q) => `<li>Queue <code>${e(q)}</code></li>`),
    ];
    out.push(
      `<section class="orphans"><h2>Orphans</h2><p>Recorded in <code>bynk.deploy.lock</code> but no longer declared. This deploy leaves them in place. To delete the KV namespaces and queues, run <code>bynk deploy --env ${env} --prune</code> in a terminal.</p><ul>${items.join("")}</ul></section>`,
    );
  }
  for (const c of plan.contexts) {
    const rows: string[] = [];
    for (const k of c.kv) rows.push(row(k.action, `KV <code>${e(k.binding)}</code> → <code>${e(k.namespace)}</code>`));
    for (const q of c.queues) rows.push(row(q.action, `queue <code>${e(q.queue)}</code>`));
    for (const d of c.durableObjects) {
      rows.push(row("advisory", `Durable Object <code>${e(d.class)}</code> (${e(d.storage)}; reconciled by ${e(d.reconciledBy)})`));
    }
    for (const s of c.secrets) {
      const note = ORIGIN_NOTE[s.origin] ?? s.origin;
      rows.push(
        row(s.action, `secret <code>${e(s.name)}</code> <span class="origin">${e(note)}</span>`, s.origin === "declared" ? "required" : ""),
      );
    }
    for (const f of c.foreignServices) {
      rows.push(row("bind", `foreign Worker <code>${e(f.worker)}</code> as <code>${e(f.binding)}</code> (not created by bynk)`));
    }
    if (c.bindsTo.length > 0) {
      rows.push(row("bind", `calls ${c.bindsTo.map((w) => `<code>${e(w)}</code>`).join(", ")}`));
    }
    const incomplete = c.secretsComplete
      ? ""
      : `<p class="incomplete">This context also reads secrets the compiler cannot name, so the list above is not everything it needs. Pass them with a secrets file.</p>`;
    out.push(
      `<section class="context"><h2><span class="action">${e(c.action)}</span> <code>${e(c.worker)}</code></h2><ul class="rows">${rows.join("")}</ul>${incomplete}</section>`,
    );
  }
  const buttons: string[] = [];
  if (opts.canDeploy) {
    buttons.push(`<button class="primary" data-action="deploy">Deploy to ${env}…</button>`);
  }
  buttons.push(`<button data-action="secretsFile">Choose secrets file…</button>`);
  buttons.push(`<button data-action="refresh">Refresh plan</button>`);
  out.push(`<footer class="actions">${buttons.join("")}</footer>`);
  if (opts.canDeploy) {
    out.push(
      `<p class="note">Deploy runs <code>${e(commandLine(deployArgs(opts.target, "deploy")))}</code> in a terminal. <code>bynk</code> asks you to confirm before it creates anything or publishes a Worker.</p>`,
    );
  }
  return out.join("\n");
}

function row(action: string, body: string, cls = ""): string {
  return `<li class="${cls}"><span class="action">${escapeHtml(action)}</span> ${body}</li>`;
}

export const PLAN_CSS = `
  .env { border-bottom: 1px solid var(--vscode-panel-border); margin-bottom: 12px; }
  .env-name { color: var(--vscode-editorWarning-foreground); }
  .action { display: inline-block; min-width: 6em; font-family: var(--vscode-editor-font-family); opacity: 0.8; }
  ul.rows { list-style: none; padding-left: 0; }
  ul.rows li { margin: 2px 0; }
  li.required { color: var(--vscode-editorWarning-foreground); }
  .origin { opacity: 0.75; font-size: 0.9em; }
  .orphans, .warnings, .incomplete { border-left: 3px solid var(--vscode-editorWarning-foreground); padding-left: 8px; }
  .warnings pre { white-space: pre-wrap; }
  footer.actions { margin-top: 16px; display: flex; gap: 8px; flex-wrap: wrap; }
  button { color: var(--vscode-button-secondaryForeground); background: var(--vscode-button-secondaryBackground); border: none; padding: 4px 12px; cursor: pointer; }
  button.primary { color: var(--vscode-button-foreground); background: var(--vscode-button-background); }
  .note { opacity: 0.8; }
`;
