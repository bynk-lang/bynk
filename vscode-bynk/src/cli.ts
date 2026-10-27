// The shared `bynk` CLI runner (#2094): one place that spawns the driver with
// the extension's settings applied, so every editor command that drives the CLI
// (`bynk dev`, and the deploy, doctor and coverage commands built on it) resolves
// `bynk` and `BYNK_BYNKC` the same way, finds the project the same way, and
// stops a process tree the same way.
//
// Two shapes: `runBynk` for a one-shot command whose output is read once it
// exits (a `--format json` document, `--format short` diagnostics), and
// `spawnBynk` for a long-running one whose output is streamed (`bynk dev`).

import { spawn, ChildProcess } from "node:child_process";
import * as vscode from "vscode";

import { contextNames } from "./devEvents";
import { bynkPath, compilerOverride } from "./tasks";

/** The contexts the project's `.bynk` sources declare, sorted: what a
 *  `--context` picker offers. */
export async function projectContexts(root: vscode.Uri): Promise<string[]> {
  const files = await vscode.workspace.findFiles(
    new vscode.RelativePattern(root, "**/*.bynk"),
    "**/{node_modules,.bynk}/**",
  );
  const names = new Set<string>();
  for (const file of files) {
    try {
      const text = new TextDecoder().decode(await vscode.workspace.fs.readFile(file));
      for (const n of contextNames(text)) names.add(n);
    } catch {
      /* unreadable: skip */
    }
  }
  return [...names].sort();
}

/** The environment a `bynk` child runs with: the extension host's, plus
 *  `BYNK_BYNKC` when `bynk.compilerPath` pins a compiler. */
export function bynkEnv(): NodeJS.ProcessEnv {
  const override = compilerOverride();
  return override ? { ...process.env, BYNK_BYNKC: override } : { ...process.env };
}

/** A finished one-shot `bynk` run. `code` is `null` when it died of a signal. */
export interface BynkResult {
  code: number | null;
  stdout: string;
  stderr: string;
}

/** Thrown when `bynk` itself could not be started (not on PATH, a bad
 *  `bynk.bynkPath`): the failure the Doctor cannot diagnose, because it runs
 *  through `bynk` too. */
export class BynkNotFoundError extends Error {
  constructor(readonly command: string, cause: unknown) {
    super(
      `could not run \`${command}\` (${String(cause)}). Install the Bynk toolchain, ` +
        "or point the `bynk.bynkPath` setting at the `bynk` executable.",
    );
  }
}

/** Run `bynk <args>` in `cwd` to completion and collect its output. A non-zero
 *  exit resolves normally (many commands use it for "found problems"); only a
 *  failure to start rejects, with a `BynkNotFoundError`. Cancelling the token
 *  stops the process tree. */
export function runBynk(
  args: string[],
  cwd: string,
  token?: vscode.CancellationToken,
  /** Text written to the child's stdin (then closed): `--sources-stdin`'s
   *  unsaved buffers. Without it stdin is ignored. */
  stdin?: string,
): Promise<BynkResult> {
  return new Promise((resolve, reject) => {
    const command = bynkPath();
    let child: ChildProcess;
    try {
      child = spawnBynk(args, cwd, { stdin: stdin === undefined ? "ignore" : "pipe" });
    } catch (e) {
      reject(new BynkNotFoundError(command, e));
      return;
    }
    if (stdin !== undefined) {
      // A child that exits before reading all of it (a bad flag) closes the
      // pipe: that is its exit code's story, not an unhandled `EPIPE`.
      child.stdin?.on("error", () => {});
      child.stdin?.end(stdin);
    }
    let stdout = "";
    let stderr = "";
    child.stdout?.setEncoding("utf8").on("data", (d: string) => (stdout += d));
    child.stderr?.setEncoding("utf8").on("data", (d: string) => (stderr += d));
    const sub = token?.onCancellationRequested(() => stopTree(child));
    child.once("error", (e) => {
      sub?.dispose();
      reject(new BynkNotFoundError(command, e));
    });
    child.once("close", (code) => {
      sub?.dispose();
      resolve({ code, stdout, stderr });
    });
  });
}

/** Spawn `bynk <args>` in `cwd` with piped stdout/stderr, in its own process
 *  group (`detached`) so `stopTree` reaches everything it starts (`bynk` →
 *  `wrangler` → `workerd`). A start failure arrives as the child's `error`. */
export function spawnBynk(
  args: string[],
  cwd: string,
  opts: { stdin?: "ignore" | "pipe" } = {},
): ChildProcess {
  return spawn(bynkPath(), args, {
    cwd,
    detached: process.platform !== "win32",
    stdio: [opts.stdin ?? "ignore", "pipe", "pipe"],
    env: bynkEnv(),
  });
}

/** Ask a `spawnBynk` child and its process group to stop (SIGTERM). `bynk dev`
 *  traps it and tears down every `wrangler` and `workerd` before exiting. */
export function stopTree(child: ChildProcess, signal: NodeJS.Signals = "SIGTERM"): void {
  if (child.exitCode !== null || child.signalCode !== null || child.pid === undefined) {
    return;
  }
  try {
    if (process.platform === "win32") {
      child.kill(signal);
    } else {
      // Negative pid: the whole process group (see `detached` above).
      process.kill(-child.pid, signal);
    }
  } catch {
    try {
      child.kill(signal);
    } catch {
      /* already gone */
    }
  }
}

/** The directory of the nearest `bynk.toml`: walking up from the active `.bynk`
 *  file, then the workspace-folder roots. Mirrors the rooting in testing.ts and
 *  debug.ts, and the LSP's `find_project_root`. */
export async function findProjectRoot(): Promise<vscode.Uri | undefined> {
  const exists = async (uri: vscode.Uri): Promise<boolean> => {
    try {
      await vscode.workspace.fs.stat(uri);
      return true;
    } catch {
      return false;
    }
  };
  const active = vscode.window.activeTextEditor?.document;
  if (active?.languageId === "bynk" && active.uri.scheme === "file") {
    let dir = vscode.Uri.joinPath(active.uri, "..");
    for (;;) {
      if (await exists(vscode.Uri.joinPath(dir, "bynk.toml"))) return dir;
      const parent = vscode.Uri.joinPath(dir, "..");
      if (parent.path === dir.path) break;
      dir = parent;
    }
  }
  for (const folder of vscode.workspace.workspaceFolders ?? []) {
    if (await exists(vscode.Uri.joinPath(folder.uri, "bynk.toml"))) return folder.uri;
  }
  return undefined;
}
