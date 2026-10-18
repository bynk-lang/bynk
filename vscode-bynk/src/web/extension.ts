// Bynk VS Code extension — the web entry point (#2102), loaded by
// `package.json`'s `"browser"` field in vscode.dev / github.dev.
//
// The desktop entry (src/extension.ts) provisions a native `bynkc-lsp`, which a
// browser can't run. Here the language server is a web worker
// (src/web/server.ts) over `bynk-wasm`, the compiler the playground already
// runs: diagnostics, hover and completion. Each workspace folder with a
// `bynk.toml` is read through `vscode.workspace.fs` and analysed as a project
// (src/web/workspace.ts); a file outside every project is analysed on its own.
// Grammar, snippets and language configuration are declarative and work
// unchanged, and inline doc rendering is client-only, so it is registered here
// too. Everything that needs a process or a local filesystem (tests, debugging,
// tasks, server download, the webview views that ask `bynk-lsp` custom
// requests) is hidden by the manifest's `!isWeb` clauses.

import * as vscode from "vscode";
import {
  BrowserMessageReader,
  BrowserMessageWriter,
  LanguageClient,
  type LanguageClientOptions,
  type MessageTransports,
} from "vscode-languageclient/browser";

import { registerInlineDocRendering } from "../inlineDocRendering";
import { bindingDelta, bindingSet, isWalkedPath, type ProjectSnapshot } from "./project";
import {
  PROJECT_FILE,
  PROJECTS,
  WASM_TRAPPED,
  type InitializationOptions,
  type ProjectFileParams,
  type ProjectsParams,
} from "./protocol";
import { loadProjects, MAX_PROJECT_FILES, readChanged, relativeTo, rootOf } from "./workspace";

/** The minimum gap between two trap-triggered worker restarts. */
const RESTART_COOLDOWN_MS = 60_000;

let client: LanguageClient | undefined;
let worker: Worker | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const output = vscode.window.createOutputChannel("Bynk LSP", { log: true });
  context.subscriptions.push(
    output,
    vscode.commands.registerCommand("bynk.showServerOutput", () => output.show()),
  );

  registerInlineDocRendering(context);

  // The projects the worker holds, by root, so a restarted worker gets them back.
  let projects: ProjectSnapshot[] = [];
  const roots = (): string[] => projects.map((p) => p.root);
  /** Each project's adapter binding modules, as last derived (by root). */
  const bindings = new Map<string, Set<string>>();

  // DECISION B: say which mode the active file is in.
  const mode = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 99);
  mode.name = "Bynk (web)";
  mode.command = "bynk.showServerOutput";
  const updateMode = (): void => {
    const doc = vscode.window.activeTextEditor?.document;
    if (!doc || doc.languageId !== "bynk") {
      mode.hide();
      return;
    }
    if (rootOf(roots(), doc.uri)) {
      mode.text = "Bynk (web)";
      mode.tooltip =
        "Bynk in the browser: diagnostics, hover and completion across this project, " +
        "from the WebAssembly compiler. Open the folder in desktop VS Code for navigation, " +
        "rename, tests and debugging.";
    } else {
      mode.text = "Bynk (web, single-file)";
      mode.tooltip =
        "This file isn't in a folder with a bynk.toml, so it is analysed on its own: names " +
        "from other units aren't resolved, and their errors are hidden.";
    }
    mode.show();
  };
  context.subscriptions.push(mode, vscode.window.onDidChangeActiveTextEditor(updateMode));

  const serverMain = vscode.Uri.joinPath(context.extensionUri, "out/web/server.js");
  const initializationOptions: InitializationOptions = {
    // DECISION C: the worker fetches this lazily, on its first request.
    wasmUrl: vscode.Uri.joinPath(context.extensionUri, "out/web/bynk_wasm_bg.wasm").toString(true),
  };
  const clientOptions: LanguageClientOptions = {
    // Any scheme: vscode.dev serves files as `vscode-vfs:`, github.dev and the
    // test host as others. The worker reads only the synced text.
    documentSelector: [{ language: "bynk" }],
    outputChannel: output,
    initializationOptions,
  };

  // A function, not a Worker, so a stop/start gets a fresh worker (and with it a
  // fresh wasm instance). The previous one is terminated first. It returns the
  // transports rather than the Worker: the client's `instanceof Worker` check
  // fails against the web extension host's own `Worker`, and the client then
  // reads the Worker as if it were the transports.
  const startWorker = (): Promise<MessageTransports> => {
    worker?.terminate();
    worker = new Worker(serverMain.toString(true));
    return Promise.resolve({
      reader: new BrowserMessageReader(worker),
      writer: new BrowserMessageWriter(worker),
    });
  };
  const lc = new LanguageClient("bynk", "Bynk (web)", startWorker, clientOptions);
  client = lc;

  const sendProjects = async (): Promise<void> => {
    const params: ProjectsParams = { projects };
    await lc.sendNotification(PROJECTS, params);
  };
  // Loads can overlap (a folder change and a manifest create, say). Only the
  // last one started is applied, whatever order they finish in.
  let generation = 0;
  const reloadProjects = async (): Promise<void> => {
    const mine = ++generation;
    const loaded = await loadProjects();
    if (mine !== generation) return;
    projects = loaded;
    bindings.clear();
    for (const p of projects) bindings.set(p.root, bindingSet(p.files));
    output.info(
      projects.length > 0
        ? `loaded ${projects.length} Bynk project(s): ${roots().join(", ")}`
        : "no bynk.toml at a workspace folder root; analysing files on their own",
    );
    updateMode();
    await sendProjects();
  };

  // A restarted worker starts empty, so it gets the projects again.
  const restart = async (): Promise<void> => {
    await lc.stop();
    await lc.start();
    await sendProjects();
  };
  // A trap restarts the worker at most once per cooldown. One inside the
  // cooldown isn't dropped: the restart is deferred to the cooldown's end. The
  // worker stops calling a trapped instance meanwhile (it answers with
  // fallbacks), so nothing is served from the poisoned instance while it waits.
  let lastRestart = 0;
  let deferred: ReturnType<typeof setTimeout> | undefined;
  const restartNow = (): void => {
    deferred = undefined;
    lastRestart = Date.now();
    output.warn("bynk-wasm trapped; restarting the web worker");
    void restart();
  };
  context.subscriptions.push(
    // Handlers registered with `onNotification` survive a stop/start.
    lc.onNotification(WASM_TRAPPED, () => {
      if (deferred) return;
      const wait = lastRestart + RESTART_COOLDOWN_MS - Date.now();
      if (wait <= 0) {
        restartNow();
      } else {
        output.warn(`bynk-wasm trapped; restarting the web worker in ${Math.ceil(wait / 1000)}s`);
        deferred = setTimeout(restartNow, wait);
      }
    }),
    { dispose: () => deferred && clearTimeout(deferred) },
  );

  // Keep the worker's copy of each project current. Edits to open files reach
  // it through document sync; these cover files changed, created or deleted on
  // disk, saves (on hosts whose watcher misses its own writes), and adapters'
  // binding modules.
  const projectOf = (uri: vscode.Uri): { root: string; project: ProjectSnapshot } | undefined => {
    const root = rootOf(roots(), uri);
    const project = projects.find((p) => p.root === root);
    return root && project ? { root, project } : undefined;
  };
  /** Record one file in the client's snapshot and send it to the worker. */
  const sendFile = async (root: string, project: ProjectSnapshot, params: ProjectFileParams) => {
    project.files = project.files.filter((f) => f.path !== params.path);
    if (params.text !== null) {
      project.files.push({ path: params.path, uri: params.uri, text: params.text });
    }
    await lc.sendNotification(PROJECT_FILE, params);
  };
  /**
   * Re-derive the binding modules the project's adapters name, from their
   * current text (`live`: an open editor's unsaved text), and read the ones
   * newly named or forget the ones no longer named.
   */
  const refreshBindings = async (
    root: string,
    project: ProjectSnapshot,
    live?: { path: string; text: string },
  ): Promise<void> => {
    const sources = project.files.map((f) => (live && f.path === live.path ? live : f));
    if (live && !sources.some((f) => f.path === live.path)) sources.push(live);
    const next = bindingSet(sources);
    const { add, drop } = bindingDelta(bindings.get(root) ?? new Set(), next);
    bindings.set(root, next);
    const base = vscode.Uri.parse(root);
    for (const path of add) {
      const uri = vscode.Uri.joinPath(base, ...path.split("/"));
      const text = await readChanged(uri);
      if (text !== null) await sendFile(root, project, { root, path, uri: uri.toString(), text });
    }
    for (const path of drop) {
      const uri = vscode.Uri.joinPath(base, ...path.split("/"));
      await sendFile(root, project, { root, path, uri: uri.toString(), text: null });
    }
  };
  const fileChanged = async (uri: vscode.Uri, deleted: boolean): Promise<void> => {
    const found = projectOf(uri);
    if (!found) return;
    const { root, project } = found;
    const path = relativeTo(root, uri);
    if (path.endsWith(".bynk")) {
      // The initial walk's rules: no skipped directory on the way, and no new
      // file past the cap.
      if (!isWalkedPath(path)) return;
      const known = project.files.some((f) => f.path === path);
      const count = project.files.filter((f) => f.path.endsWith(".bynk")).length;
      if (!deleted && !known && count >= MAX_PROJECT_FILES) {
        output.warn(`${path}: project is at ${MAX_PROJECT_FILES} .bynk files; not adding it`);
        return;
      }
    } else if (path !== "bynk.toml") {
      // A binding module: only the ones an adapter names are project files.
      if (!bindings.get(root)?.has(path)) return;
    }
    const text = deleted ? null : await readChanged(uri);
    await sendFile(root, project, { root, path, uri: uri.toString(), text });
    if (path.endsWith(".bynk")) await refreshBindings(root, project);
  };
  // Typing a `binding` line into an open adapter, before it is saved.
  const pendingBindings = new Map<string, ReturnType<typeof setTimeout>>();
  const adapterEdited = (doc: vscode.TextDocument): void => {
    if (doc.languageId !== "bynk") return;
    const found = projectOf(doc.uri);
    if (!found) return;
    const key = doc.uri.toString();
    const prior = pendingBindings.get(key);
    if (prior) clearTimeout(prior);
    pendingBindings.set(
      key,
      setTimeout(() => {
        pendingBindings.delete(key);
        const live = { path: relativeTo(found.root, doc.uri), text: doc.getText() };
        void refreshBindings(found.root, found.project, live);
      }, 300),
    );
  };
  const bynkWatcher = vscode.workspace.createFileSystemWatcher("**/*.bynk");
  const manifestWatcher = vscode.workspace.createFileSystemWatcher("**/bynk.toml");
  const bindingWatcher = vscode.workspace.createFileSystemWatcher("**/*.{ts,mts,js,mjs}");
  context.subscriptions.push(
    bynkWatcher,
    manifestWatcher,
    bindingWatcher,
    bynkWatcher.onDidCreate((uri) => void fileChanged(uri, false)),
    bynkWatcher.onDidChange((uri) => void fileChanged(uri, false)),
    bynkWatcher.onDidDelete((uri) => void fileChanged(uri, true)),
    bindingWatcher.onDidCreate((uri) => void fileChanged(uri, false)),
    bindingWatcher.onDidChange((uri) => void fileChanged(uri, false)),
    bindingWatcher.onDidDelete((uri) => void fileChanged(uri, true)),
    // A manifest appearing or going away changes which folders are projects.
    manifestWatcher.onDidCreate(() => void reloadProjects()),
    manifestWatcher.onDidDelete(() => void reloadProjects()),
    manifestWatcher.onDidChange((uri) => void fileChanged(uri, false)),
    // Not every web host's filesystem reports changes to watchers (the test
    // host's doesn't), so files created, deleted, renamed or saved through the
    // editor are routed here too. `fileChanged` ignores anything that isn't a
    // project file, so a doubled event is harmless.
    vscode.workspace.onDidSaveTextDocument((doc) => void fileChanged(doc.uri, false)),
    vscode.workspace.onDidCreateFiles((e) => e.files.forEach((uri) => void fileChanged(uri, false))),
    vscode.workspace.onDidDeleteFiles((e) => e.files.forEach((uri) => void fileChanged(uri, true))),
    vscode.workspace.onDidRenameFiles((e) =>
      e.files.forEach(({ oldUri, newUri }) => {
        void fileChanged(oldUri, true);
        void fileChanged(newUri, false);
      }),
    ),
    vscode.workspace.onDidChangeTextDocument((e) => adapterEdited(e.document)),
    vscode.workspace.onDidChangeWorkspaceFolders(() => void reloadProjects()),
    { dispose: () => pendingBindings.forEach((t) => clearTimeout(t)) },
  );

  await lc.start();
  updateMode();
  await reloadProjects();
}

export async function deactivate(): Promise<void> {
  await client?.stop();
  client = undefined;
  worker?.terminate();
  worker = undefined;
}
