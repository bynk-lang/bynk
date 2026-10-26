// #2094: run and manage `bynk dev` from the editor.
//
// One controller owns the dev session. It runs as a `bynk: dev` task, so the
// user gets a terminal they can read, the standard stop/restart, and
// `preLaunchTask` composition (DECISION B). The task uses a `CustomExecution`
// whose pseudoterminal the controller drives, not a `ShellExecution`: a shell
// task gives the extension no access to the process's output, and the
// controller needs `bynk dev --format json`'s event stream (DECISION A) to know
// which context is on which port. Everything the driver prints still reaches
// the terminal; the controller adds a marker line per lifecycle step, which the
// `$bynk-dev` problem matcher's background patterns key on.
//
// A failing rebuild runs `bynk check . --format short` and puts its diagnostics
// in Problems; the next good rebuild clears them. A dev debug session (`mode:
// "dev"`) binds the same ports, so debug.ts asks before starting one while a
// session runs (DECISION C).

import * as net from "node:net";
import type { ChildProcess } from "node:child_process";
import * as vscode from "vscode";

import { findProjectRoot, projectContexts, runBynk, spawnBynk, stopTree } from "./cli";
import { diagnosticPath, LineSplitter, parseShortDiagnostic, toTerminalText } from "./cliCore";
import {
  READY_TIMEOUT_MS,
  devArgs,
  devMarker,
  devStartMarker,
  devStatus,
  DevState,
  DevTransition,
  INITIAL_DEV_STATE,
  parseDevEvent,
  PreflightWatch,
  reduceDevState,
} from "./devEvents";
import type { LogEntry } from "./devLogs";
import type { LogLevel } from "./devEvents";
import { DevLogs } from "./devLogsView";
import { offerDoctor, reportBynkNotFound } from "./doctor";
import { bynkPath } from "./tasks";

const TASK_TYPE = "bynk";

/** The `bynk` task definition (package.json `taskDefinitions`). */
interface BynkTaskDefinition extends vscode.TaskDefinition {
  command: "dev";
  contexts?: string[];
  basePort?: number;
}

let controller: DevServerController | undefined;

/** Whether a `bynk dev` session is running (for DECISION C in debug.ts). */
export function devServerRunning(): boolean {
  return controller?.running ?? false;
}

/** Stop the running `bynk dev` session, resolving once it has exited. */
export async function stopDevServer(): Promise<void> {
  await controller?.stop();
}

export function registerDevServer(context: vscode.ExtensionContext): void {
  controller = new DevServerController();
  const c = controller;
  context.subscriptions.push(
    c,
    // #2474: the workers' logs.
    vscode.commands.registerCommand("bynk.showLogs", () => c.logs.show()),
    // With an argument, `{ minLevel, contexts? }` sets the filter directly (for
    // keybindings and tests); without, it asks.
    vscode.commands.registerCommand("bynk.filterLogs", (filter?: { minLevel?: LogLevel; contexts?: string[] }) =>
      filter
        ? c.logs.setFilter({
            minLevel: filter.minLevel ?? "info",
            contexts: filter.contexts ? new Set(filter.contexts) : undefined,
          })
        : c.logs.pickFilter(),
    ),
    vscode.commands.registerCommand("bynk.clearLogs", () => c.logs.clear()),
    // Not in package.json: the lines the view shows now, for the integration
    // tests (an output channel cannot be read back).
    vscode.commands.registerCommand("bynk.logLines", () => c.logs.lines()),
    vscode.tasks.registerTaskProvider(TASK_TYPE, {
      provideTasks: () => [c.task({ type: TASK_TYPE, command: "dev" })],
      resolveTask: (task) =>
        task.definition.type === TASK_TYPE && task.definition.command === "dev"
          ? c.task(task.definition as BynkTaskDefinition)
          : undefined,
    }),
    vscode.commands.registerCommand("bynk.startDevServer", () => c.start()),
    vscode.commands.registerCommand("bynk.stopDevServer", () => c.stop()),
    vscode.commands.registerCommand("bynk.restartDevServer", () => c.restart()),
    vscode.commands.registerCommand("bynk.devServerMenu", () => c.menu()),
  );
}

class DevServerController implements vscode.Disposable {
  /** The workers' logs, across sessions in this window (#2474). */
  readonly logs = new DevLogs();
  private session: DevSession | undefined;
  private execution: vscode.TaskExecution | undefined;
  private lastDefinition: BynkTaskDefinition = { type: TASK_TYPE, command: "dev" };
  private readonly item = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 98);
  private readonly problems = vscode.languages.createDiagnosticCollection("bynk-dev");
  private readonly subs: vscode.Disposable[] = [];
  /** Bumped per check run, so a stale `bynk check` cannot repopulate Problems
   *  after a later good rebuild cleared them. */
  private checkGeneration = 0;

  constructor() {
    this.item.command = "bynk.devServerMenu";
    this.item.name = "Bynk Dev Server";
    this.subs.push(
      vscode.tasks.onDidStartTask((e) => {
        if (isDevTask(e.execution.task)) this.execution = e.execution;
      }),
      vscode.tasks.onDidEndTask((e) => {
        if (e.execution === this.execution) this.execution = undefined;
      }),
    );
  }

  get running(): boolean {
    return this.session !== undefined && !this.session.exited;
  }

  /** The `bynk: dev` task for a definition. Built from the same definition
   *  object `resolveTask` is handed, as VS Code requires. */
  task(definition: BynkTaskDefinition): vscode.Task {
    const task = new vscode.Task(
      definition,
      vscode.TaskScope.Workspace,
      "dev",
      "bynk",
      new vscode.CustomExecution(async (resolved) =>
        this.createSession(resolved as BynkTaskDefinition),
      ),
      ["$bynk-dev"],
    );
    task.isBackground = true;
    task.presentationOptions = { reveal: vscode.TaskRevealKind.Always, panel: vscode.TaskPanelKind.Dedicated };
    return task;
  }

  private async createSession(definition: BynkTaskDefinition): Promise<vscode.Pseudoterminal> {
    if (this.running) {
      return messageTerminal(
        "A Bynk dev server is already running. Stop it (Bynk: Stop Dev Server) before starting another.",
      );
    }
    const root = await findProjectRoot();
    if (!root) {
      return messageTerminal("Bynk: no bynk.toml found in the workspace.");
    }
    const config = vscode.workspace.getConfiguration("bynk");
    const args = devArgs({
      contexts: definition.contexts,
      basePort:
        typeof definition.basePort === "number"
          ? definition.basePort
          : config.get<number | null>("dev.basePort", null),
      extraArgs: config.get<string[]>("dev.args", []),
    });
    this.lastDefinition = definition;
    this.problems.clear();
    this.logs.sessionStarted(args.slice(1).join(" "));
    const session = new DevSession(
      root,
      args,
      (prev, t, next) => this.onTransition(session, prev, t, next),
      (entry) => this.logs.push(entry),
    );
    this.session = session;
    this.render(session.state);
    return session;
  }

  private onTransition(session: DevSession, prev: DevState, t: DevTransition, next: DevState): void {
    if (session !== this.session) return;
    this.render(next);
    switch (t.event) {
      case "serving":
      case "rebuild_ok":
        this.checkGeneration++;
        this.problems.clear();
        break;
      case "rebuild_failed":
        void this.reportProblems(session.root);
        break;
      case "exited":
        // A build that fails before serving exits non-zero from `starting`.
        if (
          prev.phase === "starting" &&
          t.code !== null &&
          t.code !== 0 &&
          !session.spawnFailed &&
          !session.stopRequested
        ) {
          if (session.preflightFailed) {
            void offerDoctor("Bynk: `bynk dev` could not start: the environment is not ready for dev.", "deploy");
          } else {
            void this.reportProblems(session.root);
          }
        }
        break;
    }
  }

  /** Run `bynk check . --format short` and put its diagnostics in Problems. */
  private async reportProblems(root: vscode.Uri): Promise<void> {
    const generation = ++this.checkGeneration;
    let output: string;
    try {
      // `bynk check` writes its diagnostics to stderr; read both streams.
      const { stdout, stderr } = await runBynk(["check", ".", "--format", "short"], root.fsPath);
      output = `${stdout}\n${stderr}`;
    } catch {
      return;
    }
    if (generation !== this.checkGeneration) return;
    const byFile = new Map<string, vscode.Diagnostic[]>();
    for (const line of output.split(/\r?\n/)) {
      const d = parseShortDiagnostic(line);
      if (!d) continue;
      const uri = vscode.Uri.file(diagnosticPath(root.fsPath, d.file)).toString();
      const pos = new vscode.Position(Math.max(0, d.line - 1), Math.max(0, d.column - 1));
      const diag = new vscode.Diagnostic(
        new vscode.Range(pos, pos),
        d.message,
        d.severity === "error" ? vscode.DiagnosticSeverity.Error : vscode.DiagnosticSeverity.Warning,
      );
      diag.code = d.code;
      diag.source = "bynk dev";
      byFile.set(uri, [...(byFile.get(uri) ?? []), diag]);
    }
    this.problems.clear();
    for (const [uri, diags] of byFile) this.problems.set(vscode.Uri.parse(uri), diags);
  }

  private render(state: DevState): void {
    if (state.phase === "stopped") {
      this.item.hide();
      return;
    }
    const { text, tooltip } = devStatus(state);
    this.item.text = text;
    this.item.tooltip = tooltip;
    this.item.show();
  }

  async start(): Promise<void> {
    if (this.running) {
      const choice = await vscode.window.showInformationMessage(
        "A Bynk dev server is already running.",
        "Restart",
        "Show Terminal",
      );
      if (choice === "Restart") await this.restart();
      else if (choice === "Show Terminal") this.session?.show();
      return;
    }
    const root = await findProjectRoot();
    if (!root) {
      void vscode.window.showErrorMessage("Bynk: no bynk.toml found in the workspace.");
      return;
    }
    const contexts = await pickContexts(root);
    if (contexts === undefined) return; // cancelled
    const definition: BynkTaskDefinition = { type: TASK_TYPE, command: "dev" };
    if (contexts.length > 0) definition.contexts = contexts;
    await vscode.tasks.executeTask(this.task(definition));
  }

  async stop(): Promise<void> {
    const session = this.session;
    if (!session || session.exited) return;
    const execution = this.execution;
    // Wait for VS Code to end the task too, so a restart's new task does not
    // meet the old one still marked active.
    const taskEnded = execution
      ? new Promise<void>((resolve) => {
          const sub = vscode.tasks.onDidEndTask((e) => {
            if (e.execution === execution) {
              sub.dispose();
              resolve();
            }
          });
        })
      : Promise.resolve();
    if (execution) execution.terminate();
    else session.close();
    await session.whenExited(20_000);
    await Promise.race([taskEnded, new Promise((r) => setTimeout(r, 5_000))]);
  }

  async restart(): Promise<void> {
    const definition = this.lastDefinition;
    await this.stop();
    await vscode.tasks.executeTask(this.task(definition));
  }

  /** The status-bar quick pick: each context's URL (open, or copy), then the
   *  session actions. */
  async menu(): Promise<void> {
    const session = this.running ? this.session : undefined;
    type Item = vscode.QuickPickItem & { run?: () => unknown; url?: string };
    const copy: vscode.QuickInputButton = { iconPath: new vscode.ThemeIcon("copy"), tooltip: "Copy URL" };
    const items: Item[] = [];
    if (session) {
      for (const c of session.state.contexts) {
        items.push({ label: `$(globe) ${c.name}`, description: c.url, url: c.url, buttons: [copy] });
      }
      if (items.length > 0) items.push({ label: "", kind: vscode.QuickPickItemKind.Separator });
      items.push(
        { label: "$(terminal) Show Terminal", run: () => session.show() },
        { label: "$(output) Show Logs", run: () => this.logs.show() },
        { label: "$(debug-restart) Restart Dev Server", run: () => this.restart() },
        { label: "$(debug-stop) Stop Dev Server", run: () => this.stop() },
      );
    } else {
      items.push({ label: "$(play) Start Dev Server", run: () => this.start() });
    }
    const qp = vscode.window.createQuickPick<Item>();
    qp.items = items;
    qp.placeholder = session ? "Open a context in the browser, or manage the dev server" : "Bynk dev server";
    qp.onDidTriggerItemButton((e) => {
      if (e.item.url) {
        void vscode.env.clipboard.writeText(e.item.url);
        void vscode.window.showInformationMessage(`Copied ${e.item.url}`);
      }
      qp.hide();
    });
    qp.onDidAccept(() => {
      const picked = qp.selectedItems[0];
      qp.hide();
      if (picked?.url) void vscode.env.openExternal(vscode.Uri.parse(picked.url));
      else void picked?.run?.();
    });
    qp.onDidHide(() => qp.dispose());
    qp.show();
  }

  dispose(): void {
    this.session?.close();
    this.item.dispose();
    this.problems.dispose();
    this.logs.dispose();
    this.subs.forEach((s) => s.dispose());
  }
}

function isDevTask(task: vscode.Task): boolean {
  return task.definition.type === TASK_TYPE && task.definition.command === "dev";
}

/** One `bynk dev --format json` process, shown through a pseudoterminal. */
class DevSession implements vscode.Pseudoterminal {
  private readonly writeEmitter = new vscode.EventEmitter<string>();
  private readonly closeEmitter = new vscode.EventEmitter<number>();
  readonly onDidWrite = this.writeEmitter.event;
  readonly onDidClose = this.closeEmitter.event;

  state: DevState = INITIAL_DEV_STATE;
  exited = false;
  spawnFailed = false;
  preflightFailed = false;
  /** The user stopped the session, so a non-zero exit is no build failure. */
  stopRequested = false;
  private child: ChildProcess | undefined;
  private probe: AbortController | undefined;
  private readonly exitedPromise: Promise<void>;
  private resolveExited!: () => void;

  constructor(
    readonly root: vscode.Uri,
    private readonly args: string[],
    private readonly onTransition: (prev: DevState, t: DevTransition, next: DevState) => void,
    private readonly onLog: (entry: LogEntry) => void,
  ) {
    this.exitedPromise = new Promise((r) => (this.resolveExited = r));
  }

  open(): void {
    this.writeLine(devStartMarker(this.args));
    const child = spawnBynk(this.args, this.root.fsPath);
    this.child = child;
    const out = new LineSplitter();
    child.stdout?.setEncoding("utf8").on("data", (chunk: string) => {
      for (const line of out.push(chunk)) this.onStdoutLine(line);
    });
    const preflight = new PreflightWatch();
    child.stderr?.setEncoding("utf8").on("data", (chunk: string) => {
      // The driver's pre-flight (doctor's `deploy` gate) failed: the fix is
      // the Doctor's remedy, not a source edit.
      if (preflight.push(chunk)) this.preflightFailed = true;
      this.writeEmitter.fire(toTerminalText(chunk));
    });
    child.once("error", (e: NodeJS.ErrnoException) => {
      this.spawnFailed = true;
      this.writeLine(`Bynk: could not run \`${bynkPath()}\`: ${e.message}`);
      void reportBynkNotFound(e.code ?? e.message);
      this.finish(null);
    });
    child.once("close", (code) => {
      for (const line of out.flush()) this.onStdoutLine(line);
      this.finish(code);
    });
  }

  private onStdoutLine(line: string): void {
    const event = parseDevEvent(line);
    if (!event) {
      this.writeLine(line);
      return;
    }
    if (event.event === "log") {
      // The driver already printed the line on stderr, which the terminal shows.
      this.onLog({ context: event.context, level: event.level, text: event.text });
      return;
    }
    this.apply(event);
    if (event.event === "serving") this.probeReady();
  }

  private apply(t: DevTransition): void {
    const prev = this.state;
    const next = reduceDevState(prev, t);
    const marker = devMarker(t, prev);
    this.state = next;
    if (marker) this.writeLine(marker);
    this.onTransition(prev, t, next);
  }

  /** Poll each served port until it accepts a connection, then report `ready`
   *  (the `serving` event comes as the workers start, before they answer). */
  private probeReady(): void {
    this.probe?.abort();
    const probe = new AbortController();
    this.probe = probe;
    const ports = this.state.contexts.map((c) => c.port);
    void (async () => {
      const deadline = Date.now() + READY_TIMEOUT_MS;
      while (!probe.signal.aborted && Date.now() < deadline) {
        const up = await Promise.all(ports.map((p) => portAnswers(p)));
        if (probe.signal.aborted) return;
        if (up.every(Boolean)) {
          this.apply({ event: "ready" });
          return;
        }
        await new Promise((r) => setTimeout(r, 400));
      }
      // Give up, but end the start-up cycle: a `preLaunchTask` must not wait
      // forever on a port the probe can't reach (an `--ip` passthrough, a
      // slow first npx download, a port held elsewhere).
      if (!probe.signal.aborted) this.apply({ event: "probe_timeout" });
    })();
  }

  private finish(code: number | null): void {
    if (this.exited) return;
    this.exited = true;
    this.probe?.abort();
    this.apply({ event: "exited", code });
    this.resolveExited();
    this.closeEmitter.fire(code ?? 0);
  }

  /** The terminal was closed or the task terminated: stop the process tree. */
  close(): void {
    this.stopRequested = true;
    if (this.child && !this.exited) stopTree(this.child);
  }

  handleInput(data: string): void {
    // Ctrl-C in the dev terminal stops the session, as it would in a shell.
    if (data.includes("\x03")) this.close();
  }

  /** Reveal the dev terminal. A task's terminal is named for the task
   *  (`bynk: dev`, or `Task - dev` on older builds). */
  show(): void {
    const terminal = vscode.window.terminals.find(
      (t) => t.name.includes("bynk: dev") || t.name === "Task - dev",
    );
    if (terminal) terminal.show();
    else void vscode.commands.executeCommand("workbench.action.terminal.focus");
  }

  whenExited(timeoutMs: number): Promise<void> {
    return Promise.race([this.exitedPromise, new Promise<void>((r) => setTimeout(r, timeoutMs))]);
  }

  private writeLine(line: string): void {
    this.writeEmitter.fire(toTerminalText(`${line}\n`));
  }
}

/** Whether something accepts a TCP connection on `localhost:port`. */
function portAnswers(port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const socket = net.connect({ port, host: "localhost" });
    const done = (ok: boolean) => {
      socket.destroy();
      resolve(ok);
    };
    socket.setTimeout(1000, () => done(false));
    socket.once("connect", () => done(true));
    socket.once("error", () => done(false));
  });
}

/** A pseudoterminal that prints one message and ends, for a task that cannot run. */
function messageTerminal(message: string): vscode.Pseudoterminal {
  const write = new vscode.EventEmitter<string>();
  const close = new vscode.EventEmitter<number>();
  return {
    onDidWrite: write.event,
    onDidClose: close.event,
    open: () => {
      write.fire(toTerminalText(`${message}\n`));
      close.fire(1);
    },
    close: () => undefined,
  };
}

/** The Start quick pick: "All contexts" first, then each context the project's
 *  sources declare. `[]` serves them all; `undefined` means cancelled. A project
 *  with one context (or none found) skips the pick. */
async function pickContexts(root: vscode.Uri): Promise<string[] | undefined> {
  const names = await projectContexts(root);
  if (names.length <= 1) return [];
  const all = { label: "All contexts", description: "with the service bindings between them wired", all: true };
  const picked = await vscode.window.showQuickPick(
    [all, ...names.map((n) => ({ label: n, description: "", all: false }))],
    { placeHolder: "Which context should bynk dev serve?" },
  );
  if (!picked) return undefined;
  return picked.all ? [] : [picked.label];
}
