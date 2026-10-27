// #2101: plan-first deploy from the editor.
//
// **Bynk: Deploy…** picks an environment (from `bynk.deploy.lock`, or a new
// one) and a scope (the project, or one context), runs `bynk deploy --dry-run
// --format json`, and shows the plan: the upload order, what each context
// creates, reuses or sets, the required secrets, and any orphans. Its Deploy
// button runs the same `bynk deploy` in a terminal, without `--yes`, so the
// driver's own confirmation stays the final gate (DECISION A). **Bynk: Show
// Deploy Plan** is the preview alone and has no Deploy button.
//
// The editor never offers `--prune` (DECISION B: orphans are shown with a
// "prune from a terminal" hint) and never reads, stores or prompts for a secret
// value (DECISION C: only a `--secrets-file` path is passed through). Both rules
// live in `deployArgs` (deployPlan.ts), the one place the arguments are built.

import * as vscode from "vscode";

import { noteDriverOutput } from "./approveRequirements";
import { BynkNotFoundError, bynkEnv, findProjectRoot, projectContexts, runBynk } from "./cli";
import { isActionMessage, renderActionHtml } from "./cliView";
import {
  DeployTarget,
  deployArgs,
  lockEnvironments,
  planOutcome,
  settledState,
  validContextName,
  PLAN_CSS,
  renderPlanHtml,
  validEnvironmentName,
} from "./deployPlan";
import { reportBynkNotFound } from "./doctor";
import { bynkPath } from "./tasks";

interface PlanState {
  root: vscode.Uri;
  target: DeployTarget;
  canDeploy: boolean;
}

let panel: vscode.WebviewPanel | undefined;
let state: PlanState | undefined;
let output: vscode.OutputChannel | undefined;

export function registerDeploy(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("bynk.deploy", (preset?: Preset) => start(true, preset)),
    vscode.commands.registerCommand("bynk.showDeployPlan", (preset?: Preset) => start(false, preset)),
    { dispose: () => output?.dispose() },
  );
}

/** Command arguments that skip a picker, e.g. from a keybinding's `args`:
 *  `{ "environment": "staging", "context": "" }` (`""` is the whole project). */
interface Preset {
  environment?: string;
  context?: string;
}

async function start(canDeploy: boolean, preset?: Preset): Promise<void> {
  const root = await findProjectRoot();
  if (!root) {
    void vscode.window.showErrorMessage("Bynk: no bynk.toml found in the workspace.");
    return;
  }
  const environment =
    typeof preset?.environment === "string" && validEnvironmentName(preset.environment)
      ? preset.environment
      : await pickEnvironment(root);
  if (environment === undefined) return;
  if (typeof preset?.context === "string" && preset.context && !validContextName(preset.context)) {
    void vscode.window.showErrorMessage(
      `Bynk: ${JSON.stringify(preset.context)} is not a context name (\`commerce.orders\` or \`commerce-orders\`).`,
    );
    return;
  }
  const scope = typeof preset?.context === "string" ? preset.context : await pickScope(root);
  if (scope === undefined) return;
  const target: DeployTarget = { environment };
  if (scope) target.context = scope;
  await showPlan({ root, target, canDeploy });
}

/** Environments recorded in `bynk.deploy.lock` (`default` always first), then
 *  "New environment…". `undefined` when cancelled. */
async function pickEnvironment(root: vscode.Uri): Promise<string | undefined> {
  let recorded: string[] = [];
  try {
    const text = new TextDecoder().decode(
      await vscode.workspace.fs.readFile(vscode.Uri.joinPath(root, "bynk.deploy.lock")),
    );
    recorded = lockEnvironments(text);
  } catch {
    /* no lock yet: nothing deployed */
  }
  const names = ["default", ...recorded.filter((n) => n !== "default")];
  const NEW = "$(add) New environment…";
  const picked = await vscode.window.showQuickPick(
    [
      ...names.map((n) => ({
        label: n,
        description: recorded.includes(n) ? "recorded in bynk.deploy.lock" : "not deployed yet",
      })),
      { label: NEW, description: "" },
    ],
    { placeHolder: "Which environment should the deploy plan target?" },
  );
  if (!picked) return undefined;
  if (picked.label !== NEW) return picked.label;
  return vscode.window.showInputBox({
    prompt: "New environment name (`bynk deploy --env NAME`)",
    validateInput: (v) =>
      validEnvironmentName(v)
        ? undefined
        : "Use no spaces, and don't start with `-`.",
  });
}

/** The whole project (`""`) or one context; `undefined` when cancelled. A
 *  project with one context skips the pick. */
async function pickScope(root: vscode.Uri): Promise<string | undefined> {
  const names = await projectContexts(root);
  if (names.length <= 1) return "";
  const picked = await vscode.window.showQuickPick(
    [
      { label: "Whole project", description: "every context, in dependency order", context: "" },
      ...names.map((n) => ({
        label: n,
        description: "this context alone; the contexts it calls must already be live",
        context: n,
      })),
    ],
    { placeHolder: "What should the deploy cover?" },
  );
  return picked?.context;
}

/** Run the dry run and show (or refresh) the plan view. */
async function showPlan(next: PlanState): Promise<void> {
  const args = deployArgs(next.target, "plan");
  let result;
  let cancelled = false;
  try {
    result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: `Bynk: planning a deploy to ${next.target.environment}…`,
        cancellable: true,
      },
      (_progress, token) => {
        token.onCancellationRequested(() => (cancelled = true));
        return runBynk(args, next.root.fsPath, token);
      },
    );
  } catch (e) {
    if (e instanceof BynkNotFoundError) {
      await reportBynkNotFound();
    } else {
      void vscode.window.showErrorMessage(`Bynk: the deploy plan failed: ${String(e)}`);
    }
    return;
  }
  const outcome = planOutcome(result, cancelled);
  state = settledState(state, next, outcome);
  if (outcome.kind === "cancelled") return; // the user pressed Cancel
  if (outcome.kind === "failed") {
    // The driver refused before planning (contract skew, a cycle, a broken
    // pin, a build error…): its stderr says why and what to do. The view, if
    // open, keeps showing the last plan it had.
    output ??= vscode.window.createOutputChannel("Bynk Deploy");
    output.clear();
    output.appendLine(`$ bynk ${args.join(" ")}`);
    output.append(outcome.detail);
    noteDriverOutput(next.root.fsPath, outcome.detail);
    const choice = await vscode.window.showErrorMessage(
      `Bynk: \`bynk deploy --dry-run\` could not plan a deploy to ${next.target.environment}.`,
      "Show Output",
    );
    if (choice) output.show();
    return;
  }
  const plan = outcome.plan;
  if (!panel) {
    panel = vscode.window.createWebviewPanel("bynkDeployPlan", "Bynk Deploy Plan", vscode.ViewColumn.Active, {
      enableScripts: true,
    });
    panel.onDidDispose(() => {
      panel = undefined;
      state = undefined;
    });
    panel.webview.onDidReceiveMessage((m: unknown) => {
      if (isActionMessage(m) && state) void onAction(m.action, state);
    });
  }
  const title = `Bynk Deploy Plan: ${plan.environment}`;
  panel.title = title;
  panel.webview.html = renderActionHtml(
    panel.webview,
    title,
    renderPlanHtml(plan, { target: next.target, canDeploy: next.canDeploy, warnings: outcome.warnings }),
    PLAN_CSS,
  );
  panel.reveal();
}

async function onAction(action: string, current: PlanState): Promise<void> {
  switch (action) {
    case "refresh":
      await showPlan(current);
      break;
    case "secretsFile": {
      const picked = await vscode.window.showOpenDialog({
        defaultUri: current.root,
        canSelectMany: false,
        openLabel: "Use as secrets file",
        title: "A dotenv-style NAME=value file for `bynk deploy --secrets-file`",
      });
      if (!picked?.[0]) return;
      // Only the path is kept: the driver reads the file, the editor never does.
      await showPlan({ ...current, target: { ...current.target, secretsFile: picked[0].fsPath } });
      break;
    }
    case "deploy":
      if (current.canDeploy) await runDeploy(current);
      break;
  }
}

/** Run the real deploy in a terminal, with exactly the previewed arguments
 *  minus `--dry-run --format json`. `bynk` itself is the terminal's process, so
 *  its confirmation prompts are answered there. */
async function runDeploy(current: PlanState): Promise<void> {
  const env = current.target.environment;
  const go = `Open Terminal and Deploy`;
  const choice = await vscode.window.showWarningMessage(
    `Deploy to "${env}"?`,
    {
      modal: true,
      detail: `This runs \`bynk ${deployArgs(current.target, "deploy").join(" ")}\` in a terminal. bynk asks you to confirm there before it creates anything or publishes a Worker.`,
    },
    go,
  );
  if (choice !== go) return;
  const override = bynkEnv().BYNK_BYNKC;
  const terminal = vscode.window.createTerminal({
    name: `Bynk Deploy (${env})`,
    cwd: current.root,
    shellPath: bynkPath(),
    shellArgs: deployArgs(current.target, "deploy"),
    env: override ? { BYNK_BYNKC: override } : undefined,
  });
  terminal.show();
}
