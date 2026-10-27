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

/** One external change reported twice (watcher and editor reload) within
 *  this window triggers one refresh. */
const EXTERNAL_DEDUP_MS = 500;

export class ViewRefresh implements vscode.Disposable {
  /** One model fetch per key per refresh, shared across views. */
  readonly cache = new ModelCache();

  private readonly emitter = new vscode.EventEmitter<ReadonlySet<string>>();
  /** Fires with the URIs (strings) whose edits triggered the refresh. */
  readonly onDidRefresh = this.emitter.event;

  private readonly sourceEmitter = new vscode.EventEmitter<vscode.Uri>();
  /** Fires once per real change to a model source (#2269): an editor save, or a
   *  change made outside the editor that is neither the echo of a save nor a repeat
   *  report of one change. Unlike {@link onDidRefresh} it does not follow
   *  `bynk.views.refresh` and does not fire again as the server catches up, so a
   *  consumer that acts on a change itself (the Test Explorer's Watch toggle)
   *  reacts to each change exactly once. */
  readonly onDidChangeSource = this.sourceEmitter.event;

  private readonly clientEmitter = new vscode.EventEmitter<void>();
  /** Fires when a (re)started language client is ready (#2097's tree reloads). */
  readonly onDidStartClient = this.clientEmitter.event;
  private readonly scheduler: RefreshScheduler;
  private readonly disposables: vscode.Disposable[] = [];
  private roundTimer: ReturnType<typeof setTimeout> | undefined;
  /** When the editor last saved each file (to tell a watcher event's echo
   *  of that save from an external change). */
  private readonly saves = new Map<string, number>();
  /** When an external change to each file last triggered a refresh. */
  private readonly external = new Map<string, number>();
  /** The files external changes were last reported for (test seam). */
  private readonly seenExternal = new Set<string>();
  private readonly symbols = new Map<string, { version: number; symbols: Thenable<SymbolLike[]> }>();

  constructor(readonly getClient: () => LanguageClient | undefined) {
    this.scheduler = new RefreshScheduler((keys) => {
      this.counters.fires++;
      this.cache.invalidate();
      this.emitter.fire(keys);
    });
    this.scheduler.setMode(readMode());
    this.disposables.push(
      this.emitter,
      this.sourceEmitter,
      this.clientEmitter,
      vscode.workspace.onDidChangeConfiguration((e) => {
        if (e.affectsConfiguration("bynk.views.refresh")) this.scheduler.setMode(readMode());
      }),
      vscode.workspace.onDidSaveTextDocument((doc) => {
        if (!isModelSource(doc)) return;
        this.saves.set(doc.uri.toString(), Date.now());
        this.sourceEmitter.fire(doc.uri);
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
      // #2467: the contract diff reads both locks, so a change to either
      // (a `bynk build` refreshing the schema lock, a deploy recording the
      // ledger) refreshes it. Not model sources: `onDidChangeSource` stays
      // for source changes.
      ...this.watchLocks(),
      vscode.workspace.onDidChangeTextDocument((e) => {
        if (e.contentChanges.length === 0 || !isModelSource(e.document)) return;
        // #2144: an open, unmodified document whose file changed on disk is
        // reloaded by the editor, and that reload is a content change that
        // leaves it clean. Count it as an external change, in case the file
        // watcher doesn't report it. Undo and redo carry a `reason` and stay
        // edits. Accepted: a "Revert File" also lands here, which costs one
        // extra refresh of a model that a revert may well have changed.
        if (!e.document.isDirty && e.reason === undefined) this.externalChange(e.document.uri);
        else this.scheduler.onEdit(e.document.uri.toString());
      }),
    );
  }

  private watch(glob: string): vscode.Disposable[] {
    const w = vscode.workspace.createFileSystemWatcher(glob);
    const on = (uri: vscode.Uri) => this.externalChange(uri);
    return [w, w.onDidChange(on), w.onDidCreate(on), w.onDidDelete(on)];
  }

  private watchLocks(): vscode.Disposable[] {
    const out: vscode.Disposable[] = [];
    for (const glob of ["**/bynk.schema.lock", "**/bynk.deploy.lock"]) {
      const w = vscode.workspace.createFileSystemWatcher(glob);
      const on = (uri: vscode.Uri) => this.scheduler.onSave(uri.toString());
      out.push(w, w.onDidChange(on), w.onDidCreate(on), w.onDidDelete(on));
    }
    return out;
  }

  /** A model source changed on disk. Public as a test seam. */
  externalChange(uri: vscode.Uri): void {
    const key = uri.toString();
    const now = Date.now();
    this.counters.externalEvents++;
    this.counters.lastExternal = key;
    this.seenExternal.delete(key);
    this.seenExternal.add(key);
    if (this.seenExternal.size > 50) this.seenExternal.delete(this.seenExternal.values().next().value as string);
    prune(this.saves, now, SAVE_ECHO_MS);
    prune(this.external, now, EXTERNAL_DEDUP_MS);
    const saved = this.saves.get(key);
    if (saved !== undefined && now - saved < SAVE_ECHO_MS) {
      this.counters.echoesDropped++;
      return;
    }
    // The watcher and the editor's reload can both report one change: the
    // first triggers, and only a triggering event starts the window, so a
    // steady stream of events can't hold it open.
    const triggered = this.external.get(key);
    if (triggered !== undefined && now - triggered < EXTERNAL_DEDUP_MS) return;
    this.external.set(key, now);
    this.sourceEmitter.fire(uri);
    this.scheduler.onSave(key);
  }

  /** Test seam: whether an external change to `uri` reached the hub. */
  sawExternal(uri: string): boolean {
    return this.seenExternal.has(uri);
  }

  /** Test seam / diagnostics: what the hub has seen. */
  get stats(): {
    externalEvents: number;
    echoesDropped: number;
    fires: number;
    lastExternal: string;
    armed: boolean;
  } {
    return { ...this.counters, armed: this.scheduler.isArmed() };
  }
  private readonly counters = { externalEvents: 0, echoesDropped: 0, fires: 0, lastExternal: "" };

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

/** Drop entries older than `windowMs`, so the timestamp maps stay small. */
function prune(map: Map<string, number>, now: number, windowMs: number): void {
  for (const [k, t] of map) if (now - t >= windowMs) map.delete(k);
}

function readMode(): RefreshMode {
  return toRefreshMode(vscode.workspace.getConfiguration("bynk").get("views.refresh"));
}
