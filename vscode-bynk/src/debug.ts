// Bynk debugging (v0.72, ADR 0104) — the one-click finale of the debugging track.
//
// Per ADR 0104 D1 this is *glue, not a Debug Adapter*: the extension contributes a
// `"bynk"` debug type whose DebugConfigurationProvider compiles + starts the V8
// inspector (by shelling the `--inspect` CLIs slices 2–3 shipped), reads the
// inspector port, and hands off to VS Code's built-in JavaScript debugger
// (`pwa-node`) via a delegated *attach*. The source maps slices 1/0.70 emit — whose
// `sources` are the `.bynk` files' absolute paths (so an editor breakpoint resolves
// to the same path the debugger loads) — do the breakpoint relocation.
//
// Two runtimes, one mechanism (both proven by the integration suite):
//   - test  → `bynk test --inspect`  (Node `--inspect-brk`, type-stripped `.ts`)
//   - dev   → `bynk dev  --inspect`  (workerd via `wrangler dev --inspector-port`)

import { spawn, ChildProcess } from "node:child_process";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import * as vscode from "vscode";

import { bynkPath, compilerOverride } from "./tasks";
import { BYNK_DESCRIPTION_GENERATOR } from "./debugValues";
import { devServerRunning, stopDevServer } from "./devServer";
import { launchFailureAction } from "./cliCore";
import { driverSupport, rejectsSets } from "./testStream";
import { offerDoctor, reportBynkNotFound } from "./doctor";
import { renderBynkValue, relabelBynkLocals } from "./semanticValues";

const BYNK_TYPE = "bynk";

/** A Bynk-launched debug session, or a descendant of one — js-debug spawns a child
 *  session for the debuggee, and only the parent we configured carries the marker. */
function isBynkSession(session: vscode.DebugSession | undefined): boolean {
  return bynkKey(session) !== undefined;
}

/** The `__bynkChild` marker of a session or its nearest Bynk ancestor (the cache key). */
function bynkKey(session: vscode.DebugSession | undefined): string | undefined {
  for (let s = session; s; s = s.parentSession) {
    const k = (s.configuration as { __bynkChild?: string })?.__bynkChild;
    if (k) return k;
  }
  return undefined;
}

// Slice 3 (ADR 0105): per-session `{ emitted-fn → Bynk operation label }` maps, loaded
// from the `*.bynkdbg.json` sidecars `bynk-emit` writes, used to relabel stack frames.
const sidecarLabels = new Map<string, Map<string, string>>();

/** Recursively load every `*.bynkdbg.json` under `rootDir` and merge into one
 *  `fn → label` map. Best-effort and total — a missing/garbled sidecar is skipped, so
 *  the stack never breaks. */
async function loadSidecars(rootDir: string | undefined): Promise<Map<string, string>> {
  const labels = new Map<string, string>();
  if (!rootDir) return labels;
  const skip = new Set(["node_modules", ".git", ".wrangler"]);
  const walk = async (dir: string, depth: number): Promise<void> => {
    if (depth > 8) return;
    let entries: import("node:fs").Dirent[];
    try {
      entries = await fs.readdir(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const e of entries) {
      const p = path.join(dir, e.name);
      if (e.isDirectory()) {
        if (!skip.has(e.name)) await walk(p, depth + 1);
      } else if (e.name.endsWith(".bynkdbg.json")) {
        try {
          const obj = JSON.parse(await fs.readFile(p, "utf8")) as Record<string, unknown>;
          for (const [k, v] of Object.entries(obj)) {
            if (typeof v === "string" && !labels.has(k)) labels.set(k, v);
          }
        } catch {
          /* skip a garbled sidecar */
        }
      }
    }
  };
  await walk(rootDir, 0);
  return labels;
}

/** Whether to render Bynk values in Bynk vocabulary in the debugger (slice 5).
 *  Default on; `bynk.debug.semanticValues: false` falls back to the raw shape. */
function semanticValuesEnabled(): boolean {
  return vscode.workspace
    .getConfiguration("bynk")
    .get<boolean>("debug.semanticValues", true);
}

// Files the debugger should step *over*, not into (ADR 0103 D5): Node internals,
// the emitted runtime/glue, and wrangler's bundling scratch. Keeps stepping inside
// the user's `.bynk`-derived code rather than the machinery beneath it.
const SKIP_FILES = [
  "<node_internals>/**",
  "**/__bynk_runtime/**",
  "**/.wrangler/**",
  "**/node_modules/**",
];

export function registerDebug(context: vscode.ExtensionContext): void {
  // Each delegated `pwa-node` attach session is backed by a CLI child process
  // (bynkc/bynk → node/wrangler). Track it so terminating the debug session tears
  // the whole process group down — otherwise the inspected runtime is orphaned.
  const children = new Map<string, ChildProcess>();

  context.subscriptions.push(
    vscode.debug.registerDebugConfigurationProvider(
      BYNK_TYPE,
      new BynkDebugProvider(children),
    ),
    // The test path stops at the runtime entry (see `stopOnEntry` in the resolved
    // config) so js-debug binds pending `.bynk` breakpoints before any module
    // runs. That entry pause is the toolchain's, not the user's — auto-resume it,
    // only on our delegated sessions and only the `entry` reason, so the session
    // runs straight to the breakpoints the user set. Safe now: the entry stop
    // arrives *after* the attach is fully configured (no race), unlike a bare
    // resume on attach.
    vscode.debug.registerDebugAdapterTrackerFactory("*", {
      createDebugAdapterTracker(session: vscode.DebugSession) {
        // js-debug runs the debuggee in a *child* session (the parent is the one we
        // configured), so match the session or any ancestor carrying our marker.
        if (!isBynkSession(session)) {
          return undefined;
        }
        return {
          onDidSendMessage(m: any) {
            // (1) Auto-resume the toolchain entry pause (test path) — see above.
            if (m?.type === "event" && m.event === "stopped" && m.body?.reason === "entry") {
              void session.customRequest("continue", { threadId: m.body.threadId ?? 0 });
              return;
            }
            // (2) Slice 5/ADR 0105 D2: rewrite value previews into Bynk vocabulary,
            // editor-side, in the response stream — runtime-agnostic, so it covers
            // workerd too (where slice 5's in-debuggee generator can't run). Idempotent
            // on already-rendered values, so it composes harmlessly with the generator.
            if (m?.type !== "response" || !m.body || !semanticValuesEnabled()) return;
            if (m.command === "variables" && Array.isArray(m.body.variables)) {
              for (const v of m.body.variables) {
                if (typeof v?.value === "string") v.value = renderBynkValue(v.value); // slice 1
              }
              // Slice 2: regroup the frame into Bynk structure (capabilities, state).
              m.body.variables = relabelBynkLocals(m.body.variables);
            } else if (m.command === "evaluate" && typeof m.body.result === "string") {
              m.body.result = renderBynkValue(m.body.result);
            } else if (m.command === "stackTrace" && Array.isArray(m.body.stackFrames)) {
              // Slice 3: name a `.bynk` handler frame by its Bynk operation (`GET "/"`)
              // instead of the emitted JS function (`http_GET`), from the sidecar map.
              const labels = sidecarLabels.get(bynkKey(session) ?? "");
              if (labels) {
                for (const f of m.body.stackFrames) {
                  if (typeof f?.name === "string" && /\.bynk$/.test(f.source?.path ?? "")) {
                    const label = labels.get(f.name);
                    if (label) f.name = label;
                  }
                }
              }
            }
          },
        };
      },
    }),
    // Slice 3: load the handler-label sidecars for a Bynk session when it starts,
    // so the synchronous stackTrace relabel above has them when a breakpoint hits.
    vscode.debug.onDidStartDebugSession((s) => {
      const key = (s.configuration as { __bynkChild?: string })?.__bynkChild;
      if (key && !sidecarLabels.has(key) && semanticValuesEnabled()) {
        const cwd = (s.configuration as { cwd?: string }).cwd;
        void loadSidecars(cwd).then((m) => sidecarLabels.set(key, m));
      }
    }),
    vscode.debug.onDidTerminateDebugSession((s) => {
      const key = (s.configuration as { __bynkChild?: string })?.__bynkChild;
      if (key) {
        killChild(children, key);
        sidecarLabels.delete(key);
      }
    }),
    { dispose: () => children.forEach((_, k) => killChild(children, k)) },
  );
}

/** Start a Bynk test-debug session at `root`: shells `bynkc test --inspect`,
 *  attaches, and a breakpoint in a test body or the code it exercises pauses.
 *  The Test Explorer's Debug profile (testing.ts) routes here via `startDebugging`.
 *  v0.127: an optional `caseName` filters the run to a single case (`--case`) —
 *  the per-case `Debug Test` lens. */
export async function debugBynkTests(filter?: string | DebugFilter): Promise<void> {
  const { caseName, suites, cases } = typeof filter === "string" ? { caseName: filter } : (filter ?? {});
  const several = (suites?.length ?? 0) + (cases?.length ?? 0) > 0;
  await vscode.debug.startDebugging(undefined, {
    type: BYNK_TYPE,
    request: "launch",
    name: caseName
      ? `Debug Bynk test: ${caseName}`
      : several
        ? "Debug selected Bynk tests"
        : "Debug Bynk tests",
    mode: "test",
    caseName,
    suites,
    cases,
  });
}

/** Which tests a debug run is narrowed to: one case by name, or a set of suites and
 *  cases (a repeatable `--suite` and `--case`, which combine as "and"). */
export interface DebugFilter {
  caseName?: string;
  suites?: string[];
  cases?: string[];
}

function stringList(v: unknown): string[] | undefined {
  return Array.isArray(v) && v.every((x) => typeof x === "string") && v.length > 0 ? (v as string[]) : undefined;
}

class BynkDebugProvider implements vscode.DebugConfigurationProvider {
  constructor(private readonly children: Map<string, ChildProcess>) {}

  // A `"bynk"` config (from launch.json or `debugBynkTests`) is *resolved* into a
  // `pwa-node` attach: we launch the matching `--inspect` CLI, learn its inspector
  // port, and return a delegated Node attach. Returning a config of a different
  // type makes VS Code start *that* debugger — exactly the hand-off ADR 0104 wants.
  async resolveDebugConfiguration(
    folder: vscode.WorkspaceFolder | undefined,
    config: vscode.DebugConfiguration,
  ): Promise<vscode.DebugConfiguration | undefined> {
    // An empty `launch.json` (or a bare F5) gives us a typeless config — default
    // it to a test-debug at the workspace, the headline flow.
    const mode: "test" | "dev" = config.mode === "dev" ? "dev" : "test";
    const root = folder?.uri ?? (await findProjectRoot());
    if (!root) {
      void vscode.window.showErrorMessage(
        "Bynk debug: no bynk.toml found in the workspace.",
      );
      return undefined;
    }

    // #2094 DECISION C: a plain `bynk dev` session holds the same ports, so
    // the two are mutually exclusive. Ask, rather than fail on a port clash.
    if (mode === "dev" && devServerRunning()) {
      const stop = "Stop and Debug";
      const choice = await vscode.window.showWarningMessage(
        "A Bynk dev server is running. Stop the running dev server and debug?",
        { modal: true },
        stop,
      );
      if (choice !== stop) return undefined;
      await stopDevServer();
    }

    try {
      const { port, key } =
        mode === "dev"
          ? await this.startDev(root, config)
          : await this.startTestFiltered(root, {
              caseName: typeof config.caseName === "string" ? config.caseName : undefined,
              suites: stringList(config.suites),
              cases: stringList(config.cases),
            });
      return {
        type: "node",
        request: "attach",
        name: config.name || (mode === "dev" ? "Bynk dev" : "Bynk tests"),
        port,
        cwd: root.fsPath,
        // Stop at the runtime entry so js-debug is fully attached and tracking
        // pending breakpoints *before* any module loads — its pause-on-source-map
        // then binds a `.bynk` breakpoint as the test module parses, beating
        // execution. (Without this, `--inspect-brk` auto-resumes on attach and a
        // fast test runs to completion before the breakpoint binds.) The entry
        // pause is the toolchain's, not the user's — we auto-resume it below.
        stopOnEntry: mode === "test",
        // Resolve source maps wherever the emitted output lives (the default
        // confines resolution to the workspace folder; `.bynk/dev` and `out/` are
        // under it, but be explicit — the spike showed this is load-bearing).
        resolveSourceMapLocations: null,
        // Pre-scan the emitted output (which is `.ts`, not the `.js` js-debug
        // globs by default) so a `.bynk` breakpoint is bound *by URL* before its
        // module parses. Without this, binding waits for the script to load at
        // runtime — fine for the long-lived dev worker, but a fast test can run
        // to completion before the breakpoint binds.
        outFiles: [`${root.fsPath}/**/*.ts`, "!**/node_modules/**"],
        skipFiles: SKIP_FILES,
        // Render Bynk's tagged ADT values (`Ok(42)`, not `{tag:"Ok",…}`) — slice 5.
        // Node only: `workerd` rejects the in-debuggee evaluation this needs and
        // breaks *all* variable reading, so the dev path never sets it (the spike
        // proved both). Off when the user opts out via `bynk.debug.semanticValues`.
        ...(mode === "test" && semanticValuesEnabled()
          ? { customDescriptionGenerator: BYNK_DESCRIPTION_GENERATOR }
          : {}),
        // Tear down the CLI child when this attach session ends.
        __bynkChild: key,
      } as vscode.DebugConfiguration;
    } catch (e) {
      // #2095: a debug launch that fails on a missing tool offers the Doctor,
      // scoped to what this mode needs (Node + tsc|tsx, or wrangler).
      if (launchFailureAction(e) === "install") void reportBynkNotFound();
      else void offerDoctor(`Bynk debug: ${String(e)}`, mode === "dev" ? "deploy" : "test");
      return undefined;
    }
  }

  /** `bynk test --inspect` launches `node --inspect-brk`, which prints its
   *  `ws://host:port/…` inspector URL to stderr and pauses until we attach. We
   *  read the port from that line. Goes through `bynk` rather than shelling
   *  `bynkc` directly (#486), inheriting the driver's richer resolution. */
  /** `startTest`, falling back for a driver that predates the set filter: it rejects
   *  a `--suite` or a repeated `--case` before it starts, so the inspector never
   *  comes up. Remember that, and debug the whole project instead, saying so. */
  private async startTestFiltered(
    root: vscode.Uri,
    filter: DebugFilter,
  ): Promise<{ port: number; key: string }> {
    const isSet = (filter.suites?.length ?? 0) + (filter.cases?.length ?? 0) > 0;
    try {
      return await this.startTest(root, filter);
    } catch (e) {
      if (!isSet || !rejectsSets(String(e))) throw e;
      driverSupport.sets = false;
      void vscode.window.showInformationMessage(
        "Bynk: this `bynk` cannot filter by several tests, so the whole project runs under the inspector. " +
          "Select one test to debug just that case.",
      );
      return this.startTest(root, {});
    }
  }

  private async startTest(
    root: vscode.Uri,
    filter: DebugFilter = {},
  ): Promise<{ port: number; key: string }> {
    const args = ["test", ".", "--inspect"];
    if (filter.caseName) args.push("--case", filter.caseName);
    for (const suite of filter.suites ?? []) args.push("--suite", suite);
    for (const name of filter.cases ?? []) args.push("--case", name);
    const child = spawnCli(bynkPath(), args, root.fsPath, compilerOverride());
    const key = trackChild(this.children, child);
    try {
      const port = await waitForInspectorUrl(child, 30_000);
      return { port, key };
    } catch (e) {
      killChild(this.children, key);
      throw e;
    }
  }

  /** `bynk dev --inspect --inspector-port N` serves the worker under wrangler's V8
   *  inspector on a port we choose; we wait for its CDP discovery endpoint to come
   *  up, then attach.
   *
   *  In a multi-context project `bynk dev` serves *every* context (#552) and
   *  allocates inspector ports from `--inspect-port` upwards, so the port we pass
   *  is the first context's and that is the one we attach to. This used to fail
   *  outright — a multi-context project was an ambiguity error unless `--context`
   *  was set — so F5 now works by default. `--context` still narrows to one
   *  worker, which then takes the port itself. */
  private async startDev(
    root: vscode.Uri,
    config: vscode.DebugConfiguration,
  ): Promise<{ port: number; key: string }> {
    const port = typeof config.port === "number" ? config.port : 9229;
    const args = ["dev", "--inspect", "--inspect-port", String(port)];
    if (typeof config.context === "string" && config.context) {
      args.push("--context", config.context);
    }
    const child = spawnCli(bynkPath(), args, root.fsPath);
    const key = trackChild(this.children, child);
    try {
      await waitForInspector(port, child, 60_000);
      return { port, key };
    } catch (e) {
      killChild(this.children, key);
      throw e;
    }
  }
}

// ---------------------------------------------------------------------------
// Process + inspector plumbing
// ---------------------------------------------------------------------------

/** Spawn a CLI in its own process group (`detached`) so killing it later takes
 *  the whole tree (bynkc→node, bynk→wrangler→workerd) with it. `bynkc` is
 *  passed through as `BYNK_BYNKC` (the `bynk.compilerPath` setting) so a
 *  pinned `bynkc` still applies now that `bynk` is what gets spawned. */
function spawnCli(command: string, args: string[], cwd: string, bynkc?: string): ChildProcess {
  return spawn(command, args, {
    cwd,
    detached: true,
    stdio: ["ignore", "pipe", "pipe"],
    env: bynkc ? { ...process.env, BYNK_BYNKC: bynkc } : undefined,
  });
}

let childSeq = 0;
function trackChild(children: Map<string, ChildProcess>, child: ChildProcess): string {
  const key = `bynk-dbg-${++childSeq}`;
  children.set(key, child);
  return key;
}

function killChild(children: Map<string, ChildProcess>, key: string): void {
  const child = children.get(key);
  children.delete(key);
  if (!child || child.killed || child.pid === undefined) return;
  try {
    // Negative pid → signal the whole process group (see `detached` above).
    process.kill(-child.pid, "SIGTERM");
  } catch {
    try {
      child.kill("SIGTERM");
    } catch {
      /* already gone */
    }
  }
}

/** Resolve with the inspector port parsed from a `ws://host:port/…` line on the
 *  child's stderr (what `node --inspect-brk` prints), or reject on timeout / early
 *  exit. */
function waitForInspectorUrl(child: ChildProcess, timeoutMs: number): Promise<number> {
  return new Promise((resolve, reject) => {
    let buf = "";
    const timer = setTimeout(() => {
      cleanup();
      reject(new Error("timed out waiting for the inspector URL"));
    }, timeoutMs);
    const onData = (b: Buffer) => {
      buf += String(b);
      const m = buf.match(/ws:\/\/[^:/]+:(\d+)\//);
      if (m) {
        cleanup();
        resolve(Number(m[1]));
      }
    };
    const onExit = () => {
      cleanup();
      reject(new Error(`the inspector process exited before it was ready\n${buf}`));
    };
    // A failed spawn (`bynk` not found: ENOENT) emits `error`, not `exit`.
    const onError = (e: Error) => {
      cleanup();
      reject(e);
    };
    const cleanup = () => {
      clearTimeout(timer);
      child.stderr?.off("data", onData);
      child.off("exit", onExit);
      child.off("error", onError);
    };
    child.stderr?.on("data", onData);
    child.once("exit", onExit);
    child.once("error", onError);
  });
}

/** Resolve once wrangler's CDP discovery endpoint (`/json`) on `port` answers, or
 *  reject on timeout / early child exit. */
async function waitForInspector(
  port: number,
  child: ChildProcess,
  timeoutMs: number,
): Promise<void> {
  let exited = false;
  let spawnError: Error | undefined;
  child.once("exit", () => {
    exited = true;
  });
  // A failed spawn (`bynk` not found: ENOENT) emits `error`, not `exit`.
  child.once("error", (e) => {
    spawnError = e;
  });
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (spawnError) throw spawnError;
    if (exited) throw new Error("the dev server exited before the inspector was ready");
    try {
      const r = await fetch(`http://127.0.0.1:${port}/json`);
      const targets = (await r.json()) as unknown[];
      if (Array.isArray(targets) && targets.length > 0) return;
    } catch {
      /* not up yet */
    }
    await new Promise((r) => setTimeout(r, 400));
  }
  throw new Error(`timed out waiting for the inspector on port ${port}`);
}

/** The directory of the nearest `bynk.toml` — active `.bynk` file up, then the
 *  workspace-folder roots. Mirrors testing.ts / the LSP's `find_project_root`. */
async function findProjectRoot(): Promise<vscode.Uri | undefined> {
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
