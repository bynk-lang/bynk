// #2093: the live model panel — what turns each of the four one-shot views
// (sequence diagram #846, documentation #847, architecture map #851, wire
// contract #855) into a live preview. Each view supplies a `LiveViewSpec`
// (which request, which bundle, which scope); this module owns everything
// they share:
//
//   - the singleton panel and its click-to-code wiring (`webviewHost.ts`);
//   - refresh: the shared `ViewRefresh` hub says when (DECISION A); a panel
//     re-pulls only while visible (a hidden one re-pulls when it is shown),
//     and a response older than the latest request is dropped;
//   - the update channel (DECISION C): the first render sets `webview.html`,
//     every later one posts `{type: "update", payload, view}` into the live
//     page, so scroll position and in-page state survive;
//   - follow-cursor (DECISION B) for the handler-scoped views, retargeting
//     only when the enclosing handler changes, with an in-panel pin;
//   - restore on reload: a `WebviewPanelSerializer` re-requests from the
//     target the page persisted with `setState`.

import * as vscode from "vscode";
import type { LanguageClient } from "vscode-languageclient/node";

import {
  enclosingHandler,
  enclosingOwner,
  handlerKey,
  ownerKey,
  type Pos,
  shiftPosition,
  type SymbolLike,
} from "./refreshScheduler";
import type { ViewRefresh } from "./viewRefresh";
import {
  getNonce,
  openExternalHref,
  renderWebviewHtml,
  revealLocation,
  type WebviewMessage,
} from "./webviewHost";

/** What a view shows: a document (the project's resolution key for the
 *  architecture map) and, for a handler-scoped view, a position in it. This is
 *  what the page persists for restore. */
export interface ViewTarget {
  uri: string;
  position?: Pos;
  /** Handler-scoped views only: frozen on this handler, not following. */
  pinned?: boolean;
}

/** The view state sent alongside each payload (the page keeps it with
 *  `setState`, and draws the toolbar from it). */
export interface ViewState {
  target: ViewTarget;
  followsCursor: boolean;
  /** #2196: an owner-scoped view's handler under the cursor, for the page
   *  to highlight; absent when the cursor is outside every handler. */
  focus?: string;
  /** #2196: what an owner-scoped view follows, for the pin's wording. */
  scope?: string;
}

/** Messages a live page posts to the host, beyond click-to-code. */
type LiveMessage =
  | WebviewMessage
  | { type: "pin"; pinned: boolean }
  | { type: "refresh" }
  | { type: "ready" };

export interface LiveViewSpec<P> {
  viewType: string;
  /** "Show Sequence Diagram" — used in error and info messages. */
  commandLabel: string;
  /** The bundle under `out/` and the global it reads its payload from. */
  script: string;
  globalName: string;
  bodyCss?: string;
  /** Handler-scoped (sequence, wire contract): follow the cursor. */
  followsCursor: boolean;
  /** #2196: what cursor-follow retargets on — the enclosing handler (the
   *  default) or the enclosing service/agent (the agent state view). An
   *  owner-scoped view also reports the handler under the cursor as
   *  `ViewState.focus`, without re-requesting its model. */
  followScope?: "handler" | "owner";
  /** What an owner-scoped view follows, for the pin's wording ("agent"). */
  scopeNoun?: string;
  title(payload: P | null): string;
  /** Issue the model request; `null` when there is nothing to show. */
  request(client: LanguageClient, target: ViewTarget, refresh: ViewRefresh, fresh: boolean): Promise<P | null>;
  /** Shown when an explicit command finds nothing. */
  missMessage: string;
}

/** Debounce for cursor-follow, so a burst of selection events (arrow keys,
 *  a drag) runs one symbol lookup. */
const FOLLOW_DEBOUNCE_MS = 200;

export class LivePanel<P> implements vscode.Disposable {
  private panel: vscode.WebviewPanel | undefined;
  private target: ViewTarget | undefined;
  private handler: string | undefined;
  /** The last handler a retarget was tried for, so a handler with no model
   *  is not re-requested on every selection event inside it. */
  private tried: string | undefined;
  private lastJson: string | undefined;
  /** The payload embedded in the page's HTML — what a reloaded page shows. */
  private htmlJson: string | undefined;
  private pageLoaded = false;
  private dirty = false;
  private seq = 0;
  private followTimer: ReturnType<typeof setTimeout> | undefined;
  private readonly disposables: vscode.Disposable[] = [];
  /** Test seam counters (read through `snapshot()`). */
  private refreshes = 0;
  private nullRefreshes = 0;
  /** The cursor moved while the panel was hidden. */
  private followPending = false;
  private updates = 0;
  /** The handler under the cursor, for an owner-scoped view. */
  private focus: string | undefined;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly spec: LiveViewSpec<P>,
    private readonly hub: ViewRefresh,
  ) {
    this.disposables.push(
      hub.onDidRefresh((keys) => this.onRefresh(keys)),
      vscode.window.registerWebviewPanelSerializer(spec.viewType, {
        deserializeWebviewPanel: (panel, state) => this.restore(panel, state),
      }),
      vscode.workspace.onDidChangeTextDocument((e) => this.trackEdits(e)),
    );
    if (spec.followsCursor) {
      this.disposables.push(
        vscode.window.onDidChangeTextEditorSelection((e) => this.onSelection(e.textEditor)),
      );
    }
  }

  /** Explicit command: fetch fresh, open (or reuse) the panel and reveal it. */
  async show(target: ViewTarget): Promise<void> {
    const client = this.hub.getClient();
    if (!client) {
      void vscode.window.showErrorMessage(
        `Bynk: ${this.spec.commandLabel} needs the language server running.`,
      );
      return;
    }
    const pinned = this.target?.pinned && this.panel ? this.target.pinned : undefined;
    const next: ViewTarget = { ...target, pinned };
    const seq = ++this.seq;
    let payload: P | null;
    try {
      payload = await this.spec.request(client, next, this.hub, true);
    } catch (err) {
      void vscode.window.showErrorMessage(`Bynk: ${this.spec.commandLabel} failed: ${String(err)}`);
      return;
    }
    if (seq !== this.seq) return;
    if (!payload) {
      void vscode.window.showInformationMessage(this.spec.missMessage);
      return;
    }
    this.target = next;
    await this.rememberHandler(next);
    this.ensurePanel();
    this.render(payload);
    this.panel?.reveal(vscode.ViewColumn.Beside, true);
  }

  /** Re-pull for the current target and update the page in place. */
  async refresh(): Promise<void> {
    const panel = this.panel;
    const target = this.target;
    if (!panel || !target) return;
    if (!panel.visible) {
      this.dirty = true;
      return;
    }
    this.dirty = false;
    const seq = ++this.seq;
    this.refreshes++;
    const client = await this.hub.whenClient();
    if (!client) return;
    let payload: P | null;
    try {
      payload = await this.spec.request(client, target, this.hub, false);
    } catch {
      return; // keep showing the last good model
    }
    // A newer request (refresh, retarget or command) superseded this one, or
    // the panel closed meanwhile.
    if (seq !== this.seq || this.panel !== panel) return;
    // A target can lose its model for a moment — an edit swallowed the stored
    // position, or the file was saved mid-edit and does not parse. Keep the
    // last good model, as the throw path above does, rather than blanking the
    // view (a pinned one could not get back). Only a panel with nothing shown
    // yet (a restore) takes the empty state.
    if (payload === null) {
      this.nullRefreshes++;
      if (!this.pageLoaded) this.render(null);
      return;
    }
    this.render(payload);
  }

  /** Restore a panel VS Code kept across a window reload. */
  async restore(panel: vscode.WebviewPanel, state: unknown): Promise<void> {
    this.panel?.dispose();
    panel.webview.options = this.webviewOptions();
    this.adopt(panel);
    const target = parseTarget(state);
    if (!target) {
      this.render(null);
      return;
    }
    this.target = target;
    await this.rememberHandler(target);
    await this.refresh();
    // A restore with nothing to show (the file was deleted or renamed) still
    // needs a page: render the "nothing to show" state rather than an error.
    if (!this.pageLoaded) this.render(null);
  }

  /** Test seam: what the panel is showing. */
  snapshot(): {
    open: boolean;
    visible: boolean;
    target: ViewTarget | undefined;
    handler: string | undefined;
    refreshes: number;
    nullRefreshes: number;
    updates: number;
    hasPayload: boolean;
    focus: string | undefined;
  } {
    return {
      open: this.panel !== undefined,
      visible: this.panel?.visible ?? false,
      target: this.target,
      handler: this.handler,
      refreshes: this.refreshes,
      nullRefreshes: this.nullRefreshes,
      updates: this.updates,
      hasPayload: this.lastJson !== undefined && this.lastJson !== "null",
      focus: this.focus,
    };
  }

  /** Pin or unpin (the in-panel toggle; also a test seam). */
  setPinned(pinned: boolean): void {
    if (!this.target) return;
    this.target = { ...this.target, pinned: pinned || undefined };
    this.postView();
    if (!pinned) {
      this.tried = undefined;
      const editor = vscode.window.activeTextEditor;
      if (editor) void this.follow(editor);
    }
  }

  dispose(): void {
    if (this.followTimer !== undefined) clearTimeout(this.followTimer);
    this.panel?.dispose();
    for (const d of this.disposables) d.dispose();
  }

  private onRefresh(keys: ReadonlySet<string>): void {
    this.tried = undefined; // an edit may have given that handler a model
    if (!this.panel || !this.target || keys.size === 0) return;
    // Any model-source edit refreshes, not just one to the target file: a
    // multi-file unit's documentation page reads every file of the unit, and
    // a handler's sequence or wire contract reads consumed contexts and
    // boundary types declared elsewhere. A refresh whose payload is unchanged
    // stops at the identical-payload check, so this costs a request, not a
    // redraw.
    void this.refresh();
  }

  private onSelection(editor: vscode.TextEditor): void {
    if (!this.panel || this.target?.pinned) return;
    if (editor.document.languageId !== "bynk") return;
    if (this.followTimer !== undefined) clearTimeout(this.followTimer);
    this.followTimer = setTimeout(() => {
      this.followTimer = undefined;
      void this.follow(editor);
    }, FOLLOW_DEBOUNCE_MS);
  }

  /** Retarget to the handler under the cursor, if it is a different one. A
   *  cursor outside every handler leaves the view where it is. */
  private async follow(editor: vscode.TextEditor): Promise<void> {
    if (!this.panel || this.target?.pinned) return;
    // A hidden panel retargets when it is next shown, as `refresh` defers.
    if (!this.panel.visible) {
      this.followPending = true;
      return;
    }
    const doc = editor.document;
    if (doc.languageId !== "bynk") return;
    const pos = editor.selection.active;
    const symbols = await this.hub.documentSymbols(doc);
    const uri = doc.uri.toString();
    const key = this.scopeKey(uri, symbols, pos);
    if (!key) return;
    if (key === this.handler) {
      this.setFocus(symbols, pos);
      return;
    }
    if (key === this.tried) return;
    this.tried = key;
    const client = this.hub.getClient();
    if (!client) return;
    const target: ViewTarget = { uri, position: { line: pos.line, character: pos.character } };
    const seq = ++this.seq;
    let payload: P | null;
    try {
      payload = await this.spec.request(client, target, this.hub, false);
    } catch {
      return;
    }
    if (seq !== this.seq || !this.panel) {
      if (this.tried === key) this.tried = undefined; // superseded: may retry
      return;
    }
    // A handler with no model (a wire contract for an agent handler, say)
    // keeps the last view rather than blanking it.
    if (!payload) return;
    this.target = target;
    this.handler = key;
    this.focus = this.focusAt(symbols, pos);
    this.render(payload);
  }

  /** The follow key for `pos`: its handler, or for an owner-scoped view its
   *  service/agent. `undefined` outside any. */
  private scopeKey(uri: string, symbols: readonly SymbolLike[], pos: Pos): string | undefined {
    if (this.spec.followScope === "owner") {
      const o = enclosingOwner(symbols, pos);
      return o ? ownerKey(uri, o) : undefined;
    }
    const h = enclosingHandler(symbols, pos);
    return h ? handlerKey(uri, h) : undefined;
  }

  private focusAt(symbols: readonly SymbolLike[], pos: Pos): string | undefined {
    if (this.spec.followScope !== "owner") return undefined;
    return enclosingHandler(symbols, pos)?.symbol.name;
  }

  /** Move an owner-scoped view's highlight to the handler under the cursor. */
  private setFocus(symbols: readonly SymbolLike[], pos: Pos): void {
    const focus = this.focusAt(symbols, pos);
    if (focus === this.focus) return;
    this.focus = focus;
    this.postView();
  }

  private async rememberHandler(target: ViewTarget): Promise<void> {
    this.handler = undefined;
    this.focus = undefined;
    if (!this.spec.followsCursor || !target.position) return;
    const doc = vscode.workspace.textDocuments.find((d) => d.uri.toString() === target.uri);
    if (!doc) return;
    const symbols = await this.hub.documentSymbols(doc);
    this.handler = this.scopeKey(target.uri, symbols, target.position);
    this.focus = this.focusAt(symbols, target.position);
  }

  /** Keep a stored position on the same code as lines move above it. */
  private trackEdits(e: vscode.TextDocumentChangeEvent): void {
    const t = this.target;
    if (!t?.position || e.document.uri.toString() !== t.uri || e.contentChanges.length === 0) return;
    this.target = { ...t, position: shiftPosition(t.position, e.contentChanges) };
  }

  private webviewOptions(): vscode.WebviewOptions {
    return {
      enableScripts: true,
      localResourceRoots: [vscode.Uri.joinPath(this.context.extensionUri, "out")],
    };
  }

  private ensurePanel(): void {
    if (this.panel) return;
    const panel = vscode.window.createWebviewPanel(
      this.spec.viewType,
      this.spec.title(null),
      vscode.ViewColumn.Beside,
      { ...this.webviewOptions(), retainContextWhenHidden: true },
    );
    this.adopt(panel);
  }

  private adopt(panel: vscode.WebviewPanel): void {
    this.panel = panel;
    this.pageLoaded = false;
    this.lastJson = undefined;
    this.htmlJson = undefined;
    panel.onDidDispose(() => {
      if (this.panel !== panel) return;
      this.panel = undefined;
      this.target = undefined;
      this.handler = undefined;
      this.lastJson = undefined;
      this.pageLoaded = false;
    });
    panel.onDidChangeViewState(() => {
      if (!panel.visible) return;
      if (this.dirty) void this.refresh();
      if (this.followPending) {
        this.followPending = false;
        const editor = vscode.window.activeTextEditor ?? vscode.window.visibleTextEditors[0];
        if (editor) void this.follow(editor);
      }
    });
    panel.webview.onDidReceiveMessage((message: LiveMessage) => {
      switch (message?.type) {
        case "reveal":
          void revealLocation(message.uri, message.range);
          break;
        case "openExternal":
          void openExternalHref(message.href);
          break;
        case "pin":
          this.setPinned(message.pinned);
          break;
        case "refresh":
          void this.refresh();
          break;
        case "ready":
          this.onReady();
          break;
      }
    });
  }

  /** A page (re)loaded. Its HTML holds the payload of the first render; if
   *  updates have moved on since (the page was reloaded after being hidden,
   *  or an update raced its start-up), bring it current. */
  private onReady(): void {
    if (this.lastJson !== undefined && this.lastJson !== this.htmlJson) {
      void this.panel?.webview.postMessage({
        type: "update",
        payload: JSON.parse(this.lastJson) as unknown,
        view: this.viewState(),
      });
    } else {
      this.postView();
    }
  }

  private viewState(): ViewState {
    const view: ViewState = { target: this.target ?? { uri: "" }, followsCursor: this.spec.followsCursor };
    if (this.focus !== undefined) view.focus = this.focus;
    if (this.spec.followScope === "owner") view.scope = this.spec.scopeNoun ?? "service or agent";
    return view;
  }

  /** Show `payload`: the first render writes the page, later ones post an
   *  update into it. An identical payload is not re-sent. */
  private render(payload: P | null): void {
    const panel = this.panel;
    if (!panel) return;
    const json = JSON.stringify(payload);
    panel.title = this.spec.title(payload);
    if (!this.pageLoaded) {
      panel.webview.html = this.html(panel.webview, payload);
      this.pageLoaded = true;
      this.lastJson = json;
      this.htmlJson = json;
      return;
    }
    if (json === this.lastJson) {
      this.postView();
      return;
    }
    this.lastJson = json;
    this.updates++;
    void panel.webview.postMessage({ type: "update", payload, view: this.viewState() });
  }

  /** Re-send just the view state (a pin toggle, a moved target). */
  private postView(): void {
    void this.panel?.webview.postMessage({ type: "view", view: this.viewState() });
  }

  private html(webview: vscode.Webview, payload: P | null): string {
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.context.extensionUri, "out", this.spec.script),
    );
    return renderWebviewHtml({
      webview,
      scriptUri,
      nonce: getNonce(),
      title: this.spec.title(null),
      globalName: this.spec.globalName,
      payload,
      view: this.viewState(),
      bodyCss: this.spec.bodyCss,
    });
  }
}

/** Validate the state a page persisted (it round-trips through the webview,
 *  so treat it as untrusted shape). */
export function parseTarget(state: unknown): ViewTarget | undefined {
  if (!state || typeof state !== "object") return undefined;
  const s = state as { uri?: unknown; position?: unknown; pinned?: unknown };
  if (typeof s.uri !== "string" || s.uri === "") return undefined;
  const target: ViewTarget = { uri: s.uri };
  const p = s.position as { line?: unknown; character?: unknown } | undefined;
  if (p && typeof p.line === "number" && typeof p.character === "number") {
    target.position = { line: p.line, character: p.character };
  }
  if (s.pinned === true) target.pinned = true;
  return target;
}

/** Whether a command argument is a position (`{line, character}` numbers). */
export function isPos(value: unknown): value is Pos {
  const p = value as { line?: unknown; character?: unknown } | undefined;
  return typeof p?.line === "number" && typeof p?.character === "number";
}

/** Resolve a command's target: explicit `(uri, position)` arguments (a
 *  CodeLens, the project tree) or else the active `.bynk` editor. An
 *  `editor/title` button passes `(resourceUri, {groupId, editorIndex})` — a
 *  uri but no position — so a second argument counts only if it is a
 *  position; without one, a handler-scoped view uses the cursor of the editor
 *  showing that uri. */
export function commandTarget(
  uriArg: unknown,
  positionArg: unknown,
  withPosition: boolean,
): ViewTarget | undefined {
  const uri =
    typeof uriArg === "string" ? uriArg : uriArg instanceof vscode.Uri ? uriArg.toString() : undefined;
  if (uri !== undefined) {
    if (!withPosition) return { uri };
    if (isPos(positionArg)) {
      return { uri, position: { line: positionArg.line, character: positionArg.character } };
    }
    const shown =
      vscode.window.activeTextEditor?.document.uri.toString() === uri
        ? vscode.window.activeTextEditor
        : vscode.window.visibleTextEditors.find((e) => e.document.uri.toString() === uri);
    if (shown) {
      const pos = shown.selection.active;
      return { uri, position: { line: pos.line, character: pos.character } };
    }
    return undefined;
  }
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== "bynk") return undefined;
  const pos = editor.selection.active;
  return withPosition
    ? { uri: editor.document.uri.toString(), position: { line: pos.line, character: pos.character } }
    : { uri: editor.document.uri.toString() };
}
