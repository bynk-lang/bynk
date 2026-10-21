// A build task that type-checks the whole project through `bynk check . --format
// short`, wired to the `$bynkc` problem-matcher so errors land in the Problems
// panel. The LSP already reports diagnostics for *open* files; this catches the
// rest (unopened files, project-level errors) on demand.
//
// Goes through the `bynk` driver rather than shelling `bynkc` directly (#486):
// the driver resolves its compiler as `$BYNK_BYNKC` → PATH → sibling-of-`bynk`,
// richer than a bare PATH lookup — a driver-first install (`bynkc` reachable
// only via `BYNK_BYNKC`, or sitting next to `bynk` off PATH) now works here too.

import * as vscode from "vscode";

import { reportBynkNotFound } from "./doctor";
import { managedBynkPath } from "./toolchain";

const TASK_TYPE = "bynkc";

/** The `bynk` driver command: the `bynk.bynkPath` setting, else `bynk` on PATH,
 *  else the copy the extension downloaded (see `toolchain.ts`). */
export function bynkPath(): string {
  return (
    vscode.workspace.getConfiguration("bynk").get<string>("bynkPath", "").trim() ||
    managedBynkPath() ||
    "bynk"
  );
}

/** The `bynk.compilerPath` setting, or `undefined` when unset. Passed through as
 *  `BYNK_BYNKC` when shelling `bynk`, so the setting keeps pinning an exact
 *  `bynkc` now that `check`/`test` run through the driver instead of calling
 *  `bynkc` directly — `bynk` honours the same override. */
export function compilerOverride(): string | undefined {
  const p = vscode.workspace
    .getConfiguration("bynk")
    .get<string>("compilerPath", "")
    .trim();
  return p || undefined;
}

/** The `bynkc: check` build task: `bynk check . --format short`, run at the
 *  workspace root, errors routed through `$bynkc`. */
function checkTask(definition: vscode.TaskDefinition = { type: TASK_TYPE }): vscode.Task {
  const override = compilerOverride();
  const exec = new vscode.ShellExecution(
    bynkPath(),
    ["check", ".", "--format", "short"],
    override ? { env: { BYNK_BYNKC: override } } : undefined,
  );
  const task = new vscode.Task(
    definition,
    vscode.TaskScope.Workspace,
    "check",
    "bynkc",
    exec,
    ["$bynkc"],
  );
  task.group = vscode.TaskGroup.Build;
  return task;
}

export function registerTasks(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.tasks.registerTaskProvider(TASK_TYPE, {
      provideTasks: () => [checkTask()],
      resolveTask: (task) =>
        task.definition.type === TASK_TYPE ? checkTask(task.definition) : undefined,
    }),
    // #2095: the shell's "command not found" (127) means `bynk` itself is
    // missing, the one gap the Doctor can't diagnose, so say how to fix it.
    vscode.tasks.onDidEndTaskProcess((e) => {
      if (e.execution.task.definition.type === TASK_TYPE && e.exitCode === 127) {
        void reportBynkNotFound("command not found");
      }
    }),
  );
}
