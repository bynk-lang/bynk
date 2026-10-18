// Bynk VS Code extension — the web entry point (#2102), loaded by
// `package.json`'s `"browser"` field in vscode.dev / github.dev.
//
// The desktop entry (src/extension.ts) provisions a native `bynkc-lsp`, which a
// browser can't run. Here the language server is a web worker
// (src/web/server.ts) over `bynk-wasm`, the compiler the playground already
// runs: diagnostics, hover and completion for open files. Grammar, snippets and
// language configuration are declarative and work unchanged, and inline doc
// rendering is client-only, so it is registered here too. Everything that needs
// a process or a filesystem (tests, debugging, tasks, server download, the
// webview views that ask `bynk-lsp` custom requests) is hidden by the manifest's
// `!isWeb` clauses.

import * as vscode from "vscode";
import {
  BrowserMessageReader,
  BrowserMessageWriter,
  LanguageClient,
  type LanguageClientOptions,
  type MessageTransports,
} from "vscode-languageclient/browser";

import { registerInlineDocRendering } from "../inlineDocRendering";
import { WASM_TRAPPED, type InitializationOptions } from "./protocol";

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

  // DECISION B: say what this mode is, so a missing cross-unit name isn't read
  // as the whole story.
  const mode = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 99);
  mode.name = "Bynk (web)";
  mode.text = "Bynk (web, single-file)";
  mode.tooltip =
    "Bynk in the browser: diagnostics, hover and completion for each open file on its own, " +
    "from the WebAssembly compiler. Names from other units aren't resolved, and their errors " +
    "are hidden. Open the folder in desktop VS Code for the full language server.";
  mode.command = "bynk.showServerOutput";
  mode.show();
  context.subscriptions.push(mode);

  registerInlineDocRendering(context);

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
  client = new LanguageClient("bynk", "Bynk (web)", startWorker, clientOptions);

  // Handlers registered with `onNotification` survive a stop/start, so this
  // one keeps listening across restarts.
  const restart = async (): Promise<void> => {
    await client?.stop();
    await client?.start();
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
    client.onNotification(WASM_TRAPPED, () => {
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

  await client.start();
}

export async function deactivate(): Promise<void> {
  await client?.stop();
  client = undefined;
  worker?.terminate();
  worker = undefined;
}
