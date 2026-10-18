// Bynk VS Code extension entry point.
//
// Activates on .bynk files or workspaces containing bynk.toml. Provisions the
// bynkc-lsp language server (see server.ts: setting → PATH → cached → download)
// and connects to it as an LSP client. If no server can be provisioned the
// failure is loud and actionable (error toast + a status-bar item + commands to
// retry), rather than silently degrading to grammar-only highlighting.

import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

import {
  compareVersions,
  downloadServer,
  expectedServerVersion,
  parseVersion,
  readServerVersion,
  resolveExistingServer,
  serverVersion,
  targetTriple,
  type ResolvedServer,
} from "./server";
import { newContext, newProject } from "./scaffold";
import { registerTasks } from "./tasks";
import { registerTesting } from "./testing";
import { registerTestCodeLens } from "./testCodeLens";
import { registerDebug } from "./debug";
import { provideCodeLenses } from "./codelens";
import { registerSequenceDiagram } from "./sequenceDiagram";
import { registerDocumentationView } from "./documentationView";
import { explainClientFeature, registerExplainView } from "./explainView";
import { registerArchitectureMap } from "./architectureMap";
import { registerWireContract } from "./wireContract";
import { registerInlineDocRendering } from "./inlineDocRendering";

let client: LanguageClient | undefined;
let output: vscode.LogOutputChannel;
let projectNameItem: vscode.StatusBarItem | undefined;
let serverItem: vscode.StatusBarItem | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  // v10 of vscode-languageclient types `outputChannel` as a `LogOutputChannel`.
  output = vscode.window.createOutputChannel("Bynk LSP", { log: true });

  projectNameItem = vscode.window.createStatusBarItem(
    vscode.StatusBarAlignment.Left,
    100,
  );
  projectNameItem.command = "bynk.openProjectConfig";
  serverItem = vscode.window.createStatusBarItem(
    vscode.StatusBarAlignment.Left,
    99,
  );
  serverItem.command = "bynk.showServerOutput";
  context.subscriptions.push(projectNameItem, serverItem);

  // Commands work whether or not the server is currently running, so register
  // them before the first start attempt — that way "Restart"/"Download" are
  // available to recover from a failed start.
  context.subscriptions.push(
    vscode.commands.registerCommand("bynk.openProjectConfig", openProjectConfig),
    vscode.commands.registerCommand("bynk.showServerOutput", () => output.show()),
    vscode.commands.registerCommand("bynk.restartServer", () =>
      startServer(context, { interactive: true }),
    ),
    vscode.commands.registerCommand("bynk.downloadServer", () =>
      startServer(context, { interactive: true, forceDownload: true }),
    ),
    // Scaffolding (B-2): work without the server, so register them eagerly too.
    vscode.commands.registerCommand("bynk.newContext", () => newContext()),
    vscode.commands.registerCommand("bynk.newProject", () => newProject()),
  );

  // B-2: the `bynkc: check` build task (errors → Problems via `$bynkc`).
  registerTasks(context);

  // v0.59: the Test Explorer — runs `bynkc test --format json`. v0.78: also discovers
  // eagerly and exposes case locations for the Test CodeLens.
  const testApi = registerTesting(context);

  // v0.72: one-click debugging — the `bynk` debug type delegates to VS Code's
  // JavaScript debugger via the `--inspect` CLIs (ADR 0104).
  registerDebug(context);

  // v0.78: a `▷ Run | Debug` CodeLens above each test case (+ native gutter glyphs
  // from the eager discovery above).
  registerTestCodeLens(context, testApi);

  // #846: "Show Sequence Diagram" — a closure over the module-level `client`
  // (not the value at registration time) since the client isn't started yet
  // here and is torn down/recreated on every `startServer` call.
  registerSequenceDiagram(context, () => client);

  // #847: "Show Documentation" — the file's declarations as a rendered
  // reference page. Same client closure and shared webview substrate as the
  // sequence view above.
  registerDocumentationView(context, () => client);

  // #851: "Show Architecture Map" — the whole-project contexts/consumes/
  // capabilities map, the macro counterpart to the sequence view above.
  // Same client closure and shared webview substrate.
  registerArchitectureMap(context, () => client);

  // #855: "Show Wire Contract" — the per-handler wire-shape peek (request
  // envelope, boundary types with their re-validation, HTTP response set,
  // and — for `on call` — the cross-context contract form + hash). Same
  // client closure and shared webview substrate as the views above.
  registerWireContract(context, () => client);

  // #2098: "Explain Diagnostic Code…" and the server's "Explain …" quick fix —
  // the installed toolchain's own explanation of a code, offline, in a panel.
  registerExplainView(context, () => client);

  // #849: in-editor doc-comment rendering — heading colour, bold, italic applied
  // in place to `--- … ---` blocks. Client-side decorations only; independent of
  // the language server, so it works even while the server is still starting.
  registerInlineDocRendering(context);

  context.subscriptions.push(
    vscode.window.onDidChangeActiveTextEditor(() => updateProjectItem()),
    vscode.workspace.onDidChangeWorkspaceFolders(() => {
      updateProjectItem();
      void refreshProjectContext();
    }),
  );

  // `bynk.hasProject` gates the command-palette / explorer menus (package.json
  // `contributes.menus`) on "this workspace is a Bynk project". `when` clauses
  // cannot use the `workspaceContains:` activation predicate, so we mirror it as
  // a custom context key, kept live by a `bynk.toml` create/delete watcher.
  const tomlWatcher = vscode.workspace.createFileSystemWatcher("**/bynk.toml");
  tomlWatcher.onDidCreate(() => void refreshProjectContext());
  tomlWatcher.onDidDelete(() => void refreshProjectContext());
  context.subscriptions.push(tomlWatcher);
  void refreshProjectContext();

  await startServer(context, { interactive: false });
}

/** Set the `bynk.hasProject` context key from whether the workspace holds any
 *  `bynk.toml`. Drives the `when`-gating of the palette and explorer menus. */
async function refreshProjectContext(): Promise<void> {
  const matches = await vscode.workspace.findFiles(
    "**/bynk.toml",
    "**/node_modules/**",
    1,
  );
  await vscode.commands.executeCommand(
    "setContext",
    "bynk.hasProject",
    matches.length > 0,
  );
}

/** Provision a server, (re)start the client, and reflect the result in the UI.
 *  Safe to call repeatedly (restart command). */
async function startServer(
  context: vscode.ExtensionContext,
  opts: { interactive: boolean; forceDownload?: boolean },
): Promise<void> {
  await stopClient();

  const resolved = await ensureServer(context, opts);
  if (!resolved) {
    setServerItem("error", "Bynk LSP: not running");
    return;
  }

  const serverOptions: ServerOptions = {
    run: { command: resolved.path, transport: TransportKind.stdio },
    debug: { command: resolved.path, transport: TransportKind.stdio },
  };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "bynk" }],
    synchronize: {
      // No client-side `fileEvents` here: since v0.183 (LSP foundations slice
      // E) the server registers the `**/*.bynk`, `**/bynk.toml` and (#2081)
      // `**/bynk.schema.lock` watchers itself via
      // `workspace/didChangeWatchedFiles` dynamic registration, so it is
      // notified of source, manifest and schema-lock edits for *any* client.
      // Keeping a client-side watcher too would double-notify the server. The
      // separate `tomlWatcher` above is unrelated — it only drives the
      // `bynk.hasProject` context key and never reaches the server.
      configurationSection: "bynk",
    },
    outputChannel: output,
    middleware: {
      // Client-side gate for the server's always-on inlay hints. The master
      // `bynk.inlayHints.enable` suppresses them entirely (returning []); when
      // on, the per-kind `bynk.inlayHints.types` / `.parameterNames` toggles
      // filter the result by the server-tagged `kind`. Takes effect on the next
      // request (edit/scroll). The built-in `editor.inlayHints.enabled` is the
      // instant, editor-wide toggle — these are the persistent Bynk preferences.
      provideInlayHints: async (document, viewPort, token, next) => {
        const cfg = vscode.workspace.getConfiguration("bynk");
        if (!cfg.get<boolean>("inlayHints.enable", true)) return [];
        const showTypes = cfg.get<boolean>("inlayHints.types", true);
        const showParams = cfg.get<boolean>("inlayHints.parameterNames", true);
        const hints = await next(document, viewPort, token);
        if (!hints || (showTypes && showParams)) return hints;
        return hints.filter((h) =>
          h.kind === vscode.InlayHintKind.Type
            ? showTypes
            : h.kind === vscode.InlayHintKind.Parameter
              ? showParams
              : true,
        );
      },
      // The reference-count CodeLens carries `editor.action.showReferences`,
      // whose arguments must be real VS Code instances; the server sends them
      // as plain LSP JSON, so re-hydrate them client-side.
      provideCodeLenses,
    },
  };

  client = new LanguageClient("bynk", "Bynk LSP", serverOptions, clientOptions);
  // #2098: tells the server this client runs `bynk.explainDiagnostic`, so it
  // offers the "Explain …" quick fix.
  client.registerFeature(explainClientFeature());
  try {
    await client.start();
  } catch (e) {
    output.appendLine(`[server] failed to start: ${String(e)}`);
    void vscode.window
      .showErrorMessage(
        `Bynk: the language server failed to start (${resolved.source}). See the Bynk LSP output.`,
        "Show Output",
        "Restart",
      )
      .then((pick) => {
        if (pick === "Show Output") output.show();
        if (pick === "Restart") void startServer(context, { interactive: true });
      });
    setServerItem("error", "Bynk LSP: failed");
    return;
  }

  checkVersionMatch(context, resolved);
  setServerItem("ok", `Bynk LSP (${resolved.source})`);
  updateProjectItem();
}

/** Resolve an existing server or, when nothing is configured and the platform
 *  is supported, download one. Returns undefined (after surfacing a clear,
 *  actionable error) when no server can be provisioned. */
async function ensureServer(
  context: vscode.ExtensionContext,
  opts: { interactive: boolean; forceDownload?: boolean },
): Promise<ResolvedServer | undefined> {
  if (!opts.forceDownload) {
    const existing = resolveExistingServer(context);
    if (existing) return existing;
  }

  const configured = vscode.workspace
    .getConfiguration("bynk")
    .get<string>("executablePath", "")
    .trim();
  if (configured && !opts.forceDownload) {
    // An explicit setting that doesn't resolve: don't paper over it with a
    // download — tell the user their setting is wrong.
    await reportNoServer(
      `Bynk: \`bynk.executablePath\` is set to "${configured}", but no such executable was found.`,
      context,
    );
    return undefined;
  }

  if (!targetTriple()) {
    await reportNoServer(
      `Bynk: no prebuilt language server for ${process.platform}/${process.arch}. ` +
        "Build it with `cargo build --release -p bynk-lsp` and set `bynk.executablePath`.",
      context,
    );
    return undefined;
  }

  try {
    const path = await downloadServer(context, output);
    return { path, source: "downloaded" };
  } catch (e) {
    output.appendLine(`[server] download failed: ${String(e)}`);
    await reportNoServer(
      `Bynk: couldn't download the language server (${serverVersion(context)}). ` +
        "It may not be released yet, or the network is unavailable.",
      context,
    );
    return undefined;
  }
}

async function reportNoServer(
  message: string,
  context: vscode.ExtensionContext,
): Promise<void> {
  const pick = await vscode.window.showErrorMessage(
    message,
    "Download Server",
    "Open Settings",
    "Show Output",
  );
  if (pick === "Download Server") {
    await startServer(context, { interactive: true, forceDownload: true });
  } else if (pick === "Open Settings") {
    await vscode.commands.executeCommand(
      "workbench.action.openSettings",
      "bynk.executablePath",
    );
  } else if (pick === "Show Output") {
    output.show();
  }
}

/** Warn if the running server's version disagrees with the one this extension
 *  build expects ({@link expectedServerVersion}: the pin for a downloaded server,
 *  the extension's own version for one from PATH or the setting, #1673). A
 *  server *older* than expected (most likely a stale `bynkc-lsp` on PATH —
 *  `resolveExistingServer` prefers PATH over the pinned/downloaded copy without
 *  comparing versions, #484) gets an actionable prompt, since it can silently
 *  mis-diagnose syntax the checker already accepts. A newer or
 *  otherwise-differing server just gets a note in the output channel: the
 *  download remedy would only fetch an older one. */
function checkVersionMatch(
  context: vscode.ExtensionContext,
  resolved: ResolvedServer,
): void {
  const reported = readServerVersion(resolved.path); // "bynkc-lsp 0.23.0"
  const expected = expectedServerVersion(
    resolved.source,
    serverVersion(context),
    context.extension.packageJSON.version as string,
  ); // "0.23.0"
  if (!reported || reported.includes(expected)) return;

  output.appendLine(
    `[server] version note: running "${reported}" (${resolved.source}), extension expects ${expected}`,
  );

  const reportedVer = parseVersion(reported);
  const expectedVer = parseVersion(expected);
  const isStale =
    reportedVer && expectedVer && compareVersions(reportedVer, expectedVer) < 0;

  if (isStale && resolved.source === "setting") {
    // Forcing a download here would silently paper over the pinned setting
    // for this session only — it reverts (and re-warns) on the next reload,
    // without ever telling the user the *setting* is what's stale. Point them
    // at it instead.
    void vscode.window
      .showWarningMessage(
        `Bynk: the language server pinned by \`bynk.executablePath\` is "${reported}", ` +
          `older than the ${expected} this extension expects. It may flag valid syntax as errors.`,
        "Open Settings",
        "Ignore",
      )
      .then((pick) => {
        if (pick === "Open Settings") {
          void vscode.commands.executeCommand(
            "workbench.action.openSettings",
            "bynk.executablePath",
          );
        }
      });
  } else if (isStale && !downloadIsNewer(context, reportedVer)) {
    // #1673: a PATH server can be older than this extension build yet newer
    // than the last shipped release the download fetches, so downloading would
    // only go further back. Point at the PATH install instead.
    void vscode.window.showWarningMessage(
      `Bynk: language server is "${reported}" (${resolved.source}), older than the ` +
        `${expected} this extension expects. Rebuild or reinstall that \`bynkc-lsp\`; ` +
        "the downloadable server is older still.",
    );
  } else if (isStale) {
    void vscode.window
      .showWarningMessage(
        `Bynk: language server is "${reported}" (${resolved.source}), older than the ` +
          `${expected} this extension expects. It may flag valid syntax as errors.`,
        "Download Matching Server",
        "Ignore",
      )
      .then((pick) => {
        if (pick === "Download Matching Server") {
          void startServer(context, { interactive: true, forceDownload: true });
        }
      });
  }
  // A newer or otherwise-differing server: the version note above is all.
}

/** Whether the server a download would fetch (the pin) is newer than
 *  `reported`, i.e. whether "Download Matching Server" actually helps. */
function downloadIsNewer(
  context: vscode.ExtensionContext,
  reported: ReturnType<typeof parseVersion>,
): boolean {
  const pin = parseVersion(serverVersion(context));
  return !!reported && !!pin && compareVersions(reported, pin) < 0;
}

async function stopClient(): Promise<void> {
  if (client) {
    try {
      await client.stop();
    } catch {
      /* already down */
    }
    client = undefined;
  }
}

export async function deactivate(): Promise<void> {
  await stopClient();
}

// ---------------------------------------------------------------------------
// Status bar + project config
// ---------------------------------------------------------------------------

function setServerItem(state: "ok" | "error", text: string): void {
  if (!serverItem) return;
  const icon = state === "ok" ? "$(check)" : "$(error)";
  serverItem.text = `${icon} ${text}`;
  serverItem.tooltip =
    state === "ok"
      ? "Bynk language server is running — click to show its output"
      : "Bynk language server is not running — click to show its output";
  serverItem.backgroundColor =
    state === "error"
      ? new vscode.ThemeColor("statusBarItem.warningBackground")
      : undefined;
  updateProjectItem();
}

function updateProjectItem(): void {
  const show =
    vscode.window.activeTextEditor?.document.languageId === "bynk";
  if (!projectNameItem || !serverItem) return;
  if (!show) {
    projectNameItem.hide();
    serverItem.hide();
    return;
  }
  void readProjectName().then((name) => {
    projectNameItem!.text = `$(symbol-package) ${name ?? "no project"}`;
    projectNameItem!.tooltip = name
      ? "Open bynk.toml"
      : "No bynk.toml found in this workspace";
    projectNameItem!.show();
  });
  serverItem.show();
}

async function fileExists(uri: vscode.Uri): Promise<boolean> {
  try {
    await vscode.workspace.fs.stat(uri);
    return true;
  } catch {
    return false;
  }
}

async function findBynkToml(): Promise<vscode.Uri | undefined> {
  // Walk upward from the active `.bynk` file to the nearest `bynk.toml`,
  // mirroring the LSP's `find_project_root` (bynk-lsp/src/main.rs) — so a
  // nested project (a `bynk.toml` below the workspace-folder root) is found,
  // not just one at the root.
  const active = vscode.window.activeTextEditor?.document;
  if (active?.languageId === "bynk" && active.uri.scheme === "file") {
    let dir = vscode.Uri.joinPath(active.uri, "..");
    for (;;) {
      const candidate = vscode.Uri.joinPath(dir, "bynk.toml");
      if (await fileExists(candidate)) return candidate;
      const parent = vscode.Uri.joinPath(dir, "..");
      if (parent.path === dir.path) break; // reached the filesystem root
      dir = parent;
    }
  }
  // Fall back to the workspace-folder roots (covers a root-level project and
  // the no-active-`.bynk`-file case).
  for (const folder of vscode.workspace.workspaceFolders ?? []) {
    const candidate = vscode.Uri.joinPath(folder.uri, "bynk.toml");
    if (await fileExists(candidate)) return candidate;
  }
  return undefined;
}

async function readProjectName(): Promise<string | undefined> {
  const tomlUri = await findBynkToml();
  if (!tomlUri) return undefined;
  try {
    const buf = await vscode.workspace.fs.readFile(tomlUri);
    const text = Buffer.from(buf).toString("utf8");
    return text.match(/^\s*name\s*=\s*"([^"]+)"/m)?.[1];
  } catch {
    return undefined;
  }
}

async function openProjectConfig(): Promise<void> {
  const config = await findBynkToml();
  if (config) {
    const doc = await vscode.workspace.openTextDocument(config);
    await vscode.window.showTextDocument(doc);
  } else {
    void vscode.window.showInformationMessage(
      "No bynk.toml found in the current workspace.",
    );
  }
}
