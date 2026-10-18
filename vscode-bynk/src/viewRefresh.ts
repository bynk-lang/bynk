// #2093: the shared refresh hub for every view built on the language server's
// model requests — the four live webview panels (`livePanel.ts`) and the
// project sidebar (#2097). It turns editor events and the server's
// round-committed signal into one `onDidRefresh` event (via the pure
// `RefreshScheduler`), and owns the `ModelCache` those consumers share so one
// refresh issues one request per model, however many views show it.

import * as path from "path";

import * as vscode from "vscode";
import type { LanguageClient } from "vscode-languageclient/node";

import {
  ModelCache,
  RefreshScheduler,
  type RefreshMode,
  type SymbolLike,
  toRefreshMode,
} from "./refreshScheduler";

/** Whether a document's edits can change a model: a `.bynk` source or a
 *  project manifest. */
export function isModelSource(doc: vscode.TextDocument): boolean {
  return doc.languageId === "bynk" || path.basename(doc.uri.fsPath) === "bynk.toml";
}

/** How long to coalesce a burst of diagnostics publishes (one per file in a
 *  round) into one round-committed signal. */
const ROUND_COALESCE_MS = 100;

/** A file-watcher event this soon after the editor saved the same file is
 *  that save's echo, not an external change. */
const SAVE_ECHO_MS = 2000;

export class ViewRefresh implements vscode.Disposable {
  /** One model fetch per key per refresh, shared across views. */
  readonly cache = new ModelCache();

  private readonly emitter = new vscode.EventEmitter<ReadonlySet<string>>();
  /** Fires with the URIs (strings) whose edits triggered the refresh. */
  readonly onDidRefresh = this.emitter.event;

  private readonly clientEmitter = new vscode.EventEmitter<void>();
  /** Fires when a (re)started language client is ready (#2097's tree reloads). */
  readonly onDidStartClient = this.clientEmitter.event;
  private readonly scheduler: RefreshScheduler;
  private readonly disposables: vscode.Disposable[] = [];
  private roundTimer: ReturnType<typeof setTimeout> | undefined;
  /** When the editor last saved each file (to tell a watcher event's echo
   *  of that save from an external change). */
  private readonly saves = new Map<string, number>();
  private readonly symbols = new Map<string, { version: number; symbols: Thenable<SymbolLike[]> }>();

  constructor(readonly getClient: () => LanguageClient | undefined) {
    this.scheduler = new RefreshScheduler((keys) => {
      this.cache.invalidate();
      this.emitter.fire(keys);
    });
    this.scheduler.setMode(readMode());
    this.disposables.push(
      this.emitter,
      this.clientEmitter,
      vscode.workspace.onDidChangeConfiguration((e) => {
        if (e.affectsConfiguration("bynk.views.refresh")) this.scheduler.setMode(readMode());
      }),
      vscode.workspace.onDidSaveTextDocument((doc) => {
        if (!isModelSource(doc)) return;
        this.saves.set(doc.uri.toString(), Date.now());
        this.scheduler.onSave(doc.uri.toString());
      }),
      vscode.workspace.onDidCloseTextDocument((doc) => this.symbols.delete(doc.uri.toString())),
      // #2112 review: changes made outside the editor (a git checkout,
      // codegen, a file created or deleted) go through the same scheduler as
      // a save, so `bynk.views.refresh` holds (nothing under `manual`) and
      // the model cache is invalidated when it fires. A watcher event that is
      // just the echo of an editor save is dropped: the save already counted.
      ...this.watch("**/*.bynk"),
      ...this.watch("**/bynk.toml"),
      vscode.workspace.onDidChangeTextDocument((e) => {
        if (e.contentChanges.length > 0 && isModelSource(e.document)) {
          this.scheduler.onEdit(e.document.uri.toString());
        }
      }),
    );
  }

  private watch(glob: string): vscode.Disposable[] {
    const w = vscode.workspace.createFileSystemWatcher(glob);
    const on = (uri: vscode.Uri) => this.externalChange(uri);
    return [w, w.onDidChange(on), w.onDidCreate(on), w.onDidDelete(on)];
  }

  /** A model source changed on disk. Public as a test seam. */
  externalChange(uri: vscode.Uri): void {
    const key = uri.toString();
    const saved = this.saves.get(key);
    if (saved !== undefined && Date.now() - saved < SAVE_ECHO_MS) return;
    this.scheduler.onSave(key);
  }

  get mode(): RefreshMode {
    return this.scheduler.getMode();
  }

  /** The language server published diagnostics: a round just committed. Wired
   *  from the client's `handleDiagnostics` middleware in `extension.ts`. */
  roundCommitted(): void {
    if (this.roundTimer !== undefined) return;
    this.roundTimer = setTimeout(() => {
      this.roundTimer = undefined;
      this.scheduler.onRoundCommitted();
    }, ROUND_COALESCE_MS);
  }

  /** A (re)started language client is ready to take requests. */
  clientStarted(): void {
    this.clientEmitter.fire();
  }

  /** The current client, or the next one to start (within `timeoutMs`). A
   *  panel restored after a window reload asks before the server is up. */
  async whenClient(timeoutMs = 30_000): Promise<LanguageClient | undefined> {
    const now = this.getClient();
    if (now) return now;
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        sub.dispose();
        resolve(this.getClient());
      }, timeoutMs);
      const sub = this.clientEmitter.event(() => {
        clearTimeout(timer);
        sub.dispose();
        resolve(this.getClient());
      });
    });
  }

  /** A document's symbol tree, cached per document version. Used to find the
   *  handler under the cursor (cursor-follow) and handler CodeLens anchors. */
  async documentSymbols(doc: vscode.TextDocument): Promise<SymbolLike[]> {
    // One entry per document, replaced when its version moves on, so the
    // cache stays bounded however many edits pass (and whatever the refresh
    // mode: the model cache is only cleared when a refresh fires).
    const uri = doc.uri.toString();
    const hit = this.symbols.get(uri);
    if (hit && hit.version === doc.version) return hit.symbols;
    const symbols = vscode.commands
      .executeCommand<vscode.DocumentSymbol[] | undefined>("vscode.executeDocumentSymbolProvider", doc.uri)
      // A provider may answer with flat `SymbolInformation`s, which carry no
      // `children`/`selectionRange`; only the hierarchical form is usable here.
      .then((result) => (result ?? []).filter((s) => "selectionRange" in s) as SymbolLike[]);
    this.symbols.set(uri, { version: doc.version, symbols });
    symbols.then(undefined, () => {
      if (this.symbols.get(uri)?.symbols === symbols) this.symbols.delete(uri);
    });
    return symbols;
  }

  dispose(): void {
    if (this.roundTimer !== undefined) clearTimeout(this.roundTimer);
    this.scheduler.dispose();
    for (const d of this.disposables) d.dispose();
  }
}

function readMode(): RefreshMode {
  return toRefreshMode(vscode.workspace.getConfiguration("bynk").get("views.refresh"));
}
