// #2468: the State Migrations view — `bynk migrate status` in the Project
// sidebar, and a plan-first sweep.
//
// The tree node (projectTree.ts) reads this store: one `bynk migrate status
// --format json` per project, cached until a refresh (DECISION C: offline, the
// editor never reaches Cloudflare). **Bynk: Sweep State Migrations…** picks the
// environment and scope, shows the sweep plan, and its Sweep button asks for a
// typed confirmation when production is involved, then runs `bynk migrate
// sweep` in a terminal without `--yes`, so the driver's own prompt stays the
// final gate (DECISION B, as for deploy: ADR 0510). The plan is the driver's
// status report; the instance counts come from the account, so the driver
// prints them in the terminal before it asks.

import * as vscode from "vscode";

import { BynkNotFoundError, bynkEnv, findProjectRoot, runBynk } from "./cli";
import { isActionMessage, renderActionHtml } from "./cliView";
import { PLAN_CSS, validContextName, validEnvironmentName } from "./deployPlan";
import { reportBynkNotFound } from "./doctor";
import {
  type ClassStatus,
  contextOf,
  isProductionEnvironment,
  renderSweepPlanHtml,
  statusOutcome,
  sweepArgs,
  sweptEnvironments,
  type SweepTarget,
  typedConfirmation,
} from "./migrationsCore";
import { bynkPath } from "./tasks";

/** A project's status: the classes, or why the driver could not report them. */
export type MigrationsState =
  | { kind: "status"; classes: ClassStatus[] }
  | { kind: "failed"; detail: string }
  | { kind: "missing" };

export class MigrationsStore implements vscode.Disposable {
  private readonly emitter = new vscode.EventEmitter<void>();
  readonly onDidChange = this.emitter.event;
  private readonly states = new Map<string, MigrationsState>();
  private readonly pending = new Map<string, Promise<MigrationsState>>();

  /** The cached state, loading it the first time. */
  get(root: vscode.Uri): Promise<MigrationsState> {
    const key = root.toString();
    const cached = this.states.get(key);
    if (cached) return Promise.resolve(cached);
    return this.refresh(root);
  }

  /** Run `bynk migrate status` again and tell the tree. */
  refresh(root: vscode.Uri): Promise<MigrationsState> {
    const key = root.toString();
    const running = this.pending.get(key);
    if (running) return running;
    const run = this.load(root).then((state) => {
      this.states.set(key, state);
      this.pending.delete(key);
      this.emitter.fire();
      return state;
    });
    this.pending.set(key, run);
    return run;
  }

  /** Forget the cached states: the next read runs `bynk migrate status` again. */
  clear(): void {
    this.states.clear();
  }

  /** Forget the cached states and repaint, e.g. after a sweep changed the lock. */
  invalidate(): void {
    this.clear();
    this.emitter.fire();
  }

  dispose(): void {
    this.emitter.dispose();
  }

  private async load(root: vscode.Uri): Promise<MigrationsState> {
    try {
      const result = await runBynk(["migrate", "status", "--format", "json"], root.fsPath);
      return statusOutcome(result);
    } catch (e) {
      if (e instanceof BynkNotFoundError) return { kind: "missing" };
      return { kind: "failed", detail: String(e) };
    }
  }
}

/** Command arguments that skip a picker, e.g. from a keybinding's `args`:
 *  `{ "environment": "staging", "context": "" }` (`""` is the whole project;
 *  an omitted `environment` sweeps every recorded environment). */
interface Preset {
  environment?: string;
  context?: string;
}

interface PlanState {
  root: vscode.Uri;
  target: SweepTarget;
}

let panel: vscode.WebviewPanel | undefined;
let planned: PlanState | undefined;
let storeRef: MigrationsStore | undefined;

export function registerMigrations(context: vscode.ExtensionContext, store: MigrationsStore): void {
  storeRef = store;
  context.subscriptions.push(
    store,
    vscode.commands.registerCommand("bynk.migrate.refresh", async () => {
      const roots = await migrationRoots();
      await Promise.all(roots.map((r) => store.refresh(r)));
    }),
    vscode.commands.registerCommand("bynk.migrate.sweep", (arg?: unknown) => startSweep(store, arg)),
    { dispose: () => panel?.dispose() },
  );
}

async function migrationRoots(): Promise<vscode.Uri[]> {
  const root = await findProjectRoot();
  return root ? [root] : [];
}

const rec = (v: unknown): Record<string, unknown> =>
  v && typeof v === "object" ? (v as Record<string, unknown>) : {};

/** A tree node or command argument naming a project (`{ root }`), a class
 *  (`{ root, status: { class } }`), or a preset. */
function parseArg(arg: unknown): { root?: vscode.Uri; preset: Preset; cls?: string } {
  const a = (arg && typeof arg === "object" ? arg : {}) as Record<string, unknown>;
  const root = a.root instanceof vscode.Uri ? a.root : undefined;
  const preset: Preset = {};
  if (typeof a.environment === "string") preset.environment = a.environment;
  if (typeof a.context === "string") preset.context = a.context;
  const status = rec(a.status);
  return { root, preset, cls: typeof status.class === "string" ? status.class : undefined };
}

async function startSweep(store: MigrationsStore, arg: unknown): Promise<void> {
  const parsed = parseArg(arg);
  const root = parsed.root ?? (await findProjectRoot());
  if (!root) {
    void vscode.window.showErrorMessage("Bynk: no bynk.toml found in the workspace.");
    return;
  }
  const state = await store.refresh(root);
  if (state.kind === "missing") {
    await reportBynkNotFound();
    return;
  }
  if (state.kind === "failed") {
    void vscode.window.showErrorMessage(
      `Bynk: \`bynk migrate status\` could not report: ${state.detail.split("\n")[0] || "no output"}`,
    );
    return;
  }
  const classes = state.classes;
  if (classes.length === 0) {
    void vscode.window.showInformationMessage("Bynk: no agent classes are recorded in bynk.schema.lock; nothing to sweep.");
    return;
  }
  const { preset } = parsed;
  if (preset.environment !== undefined && !validEnvironmentName(preset.environment)) {
    void vscode.window.showErrorMessage(`Bynk: ${JSON.stringify(preset.environment)} is not an environment name.`);
    return;
  }
  if (preset.context && !validContextName(preset.context)) {
    void vscode.window.showErrorMessage(
      `Bynk: ${JSON.stringify(preset.context)} is not a context name (\`commerce.orders\` or \`commerce-orders\`).`,
    );
    return;
  }
  const scope = preset.context ?? (parsed.cls ? contextOf(parsed.cls) : await pickScope(classes));
  if (scope === undefined) return;
  const target: SweepTarget = { environments: [] };
  if (scope) target.context = scope;
  let environment: string | undefined = preset.environment;
  if (environment === undefined) {
    // A preset that names only a context sweeps every recorded environment.
    environment = preset.context !== undefined ? "" : await pickEnvironment(classes, target);
    if (environment === undefined) return; // cancelled
  }
  if (environment) target.environments = [environment];
  await showPlan({ root, target }, classes);
}

/** The whole project (`""`) or one context; `undefined` when cancelled. A
 *  project with one context skips the pick. */
async function pickScope(classes: ClassStatus[]): Promise<string | undefined> {
  const names = [...new Set(classes.map((c) => contextOf(c.class)))].sort();
  if (names.length <= 1) return "";
  const picked = await vscode.window.showQuickPick(
    [
      { label: "Whole project", description: "every context's classes", context: "" },
      ...names.map((n) => ({ label: n, description: "this context's classes alone", context: n })),
    ],
    { placeHolder: "What should the sweep cover?" },
  );
  return picked?.context;
}

/** One environment, or `""` for every recorded one; `undefined` when cancelled. */
async function pickEnvironment(classes: ClassStatus[], target: SweepTarget): Promise<string | undefined> {
  const recorded = sweptEnvironments(classes, target);
  const ALL = "Every recorded environment";
  const picked = await vscode.window.showQuickPick(
    [
      { label: ALL, description: recorded.join(", ") || "none recorded", environment: "" },
      ...recorded.map((n) => ({
        label: n,
        description: "migrates but raises no floor unless it is the only environment recorded",
        environment: n,
      })),
    ],
    { placeHolder: "Which environment should the sweep cover?" },
  );
  return picked?.environment;
}

/** Show (or refresh) the sweep plan view. */
async function showPlan(next: PlanState, classes: ClassStatus[]): Promise<void> {
  planned = next;
  const envs = sweptEnvironments(classes, next.target);
  const confirmation = typedConfirmation(envs);
  if (!panel) {
    panel = vscode.window.createWebviewPanel("bynkSweepPlan", "Bynk Sweep Plan", vscode.ViewColumn.Active, {
      enableScripts: true,
    });
    panel.onDidDispose(() => {
      panel = undefined;
      planned = undefined;
    });
    panel.webview.onDidReceiveMessage((m: unknown) => {
      if (isActionMessage(m) && planned) void onAction(m.action, planned);
    });
  }
  const title = `Bynk Sweep Plan: ${next.target.environments[0] ?? "all environments"}`;
  panel.title = title;
  panel.webview.html = renderActionHtml(
    panel.webview,
    title,
    renderSweepPlanHtml(classes, { target: next.target, confirmation }),
    PLAN_CSS,
  );
  panel.reveal();
}

async function onAction(action: string, current: PlanState): Promise<void> {
  if (action === "refresh") {
    const state = storeRef ? await storeRef.refresh(current.root) : undefined;
    if (state?.kind === "status") await showPlan(current, state.classes);
  } else if (action === "sweep") {
    await runSweep(current);
  }
}

/** The typed confirmation (production only), then a modal, then the terminal. */
async function runSweep(current: PlanState): Promise<void> {
  const state = storeRef ? await storeRef.get(current.root) : undefined;
  const classes = state?.kind === "status" ? state.classes : [];
  const envs = sweptEnvironments(classes, current.target);
  const phrase = typedConfirmation(envs);
  if (phrase) {
    const typed = await vscode.window.showInputBox({
      title: "Sweep production?",
      prompt: `This sweep touches ${envs.filter(isProductionEnvironment).join(", ")}. Type "${phrase}" to continue.`,
      validateInput: (v) => (v === phrase ? undefined : `Type ${phrase} exactly.`),
    });
    if (typed !== phrase) return;
  }
  const args = sweepArgs(current.target);
  const go = "Open Terminal and Sweep";
  const choice = await vscode.window.showWarningMessage(
    `Sweep ${envs.length > 0 ? envs.join(", ") : "the recorded environments"}?`,
    {
      modal: true,
      detail: `This runs \`bynk ${args.join(" ")}\` in a terminal. bynk lists the instances and asks you to confirm there before it changes anything on your Cloudflare account.`,
    },
    go,
  );
  if (choice !== go) return;
  const override = bynkEnv().BYNK_BYNKC;
  const terminal = vscode.window.createTerminal({
    name: `Bynk Sweep (${envs.join(", ") || "all"})`,
    cwd: current.root,
    shellPath: bynkPath(),
    shellArgs: args,
    env: override ? { BYNK_BYNKC: override } : undefined,
  });
  terminal.show();
  // The lock may change when the run ends: refresh when the terminal closes.
  const sub = vscode.window.onDidCloseTerminal((t) => {
    if (t !== terminal) return;
    sub.dispose();
    storeRef?.invalidate();
  });
}
