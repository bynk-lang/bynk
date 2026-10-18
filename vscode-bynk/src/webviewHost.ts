// #847: the shared webview substrate, extracted from #846's sequence-diagram
// panel so the documentation view is its second consumer rather than a second
// copy (issue #847's Framing: "built once, two consumers"). Host-side only —
// this module imports `vscode`, so it must never be pulled into a webview
// bundle (the browser-context entries under `webview/` cannot import `vscode`).
//
// What's shared: the strict CSP + per-render nonce, the payload-embedding HTML
// shell, and `postMessage`→reveal click-to-code. The panel lifecycle
// (singleton panel, live refresh, cursor-follow, restore on reload) is shared
// too, in `livePanel.ts` (#2093). What stays per-feature: the request, the
// payload and the vendored renderer bundle (Mermaid for #846, markdown-it).

import * as vscode from "vscode";

/** A serialisable range a webview posts back for click-to-code (never a real
 *  `vscode.Range` — a webview has no access to the `vscode` module). */
export interface RevealRange {
  start: { line: number; character: number };
  end: { line: number; character: number };
}

/** The `reveal` message shape every Bynk webview posts to the host. */
export interface RevealMessage {
  type: "reveal";
  uri: string;
  range: RevealRange;
}

/** A `openExternal` message: a webview asks the host to open an http(s) URL
 *  from rendered doc-comment Markdown. The host validates the scheme before
 *  handing it to `vscode.env.openExternal` (which shows its own trust prompt);
 *  see #847's link-handling decision. */
export interface OpenExternalMessage {
  type: "openExternal";
  href: string;
}

export type WebviewMessage = RevealMessage | OpenExternalMessage;

/** A fresh 32-char nonce for the render's `script-src 'nonce-…'`. */
export function getNonce(): string {
  const possible = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let text = "";
  for (let i = 0; i < 32; i++) {
    text += possible.charAt(Math.floor(Math.random() * possible.length));
  }
  return text;
}

/** Open `uriStr` and select `range` — the host end of click-to-code, shared
 *  by every Bynk webview. Reveals in the first editor column, focused. */
export async function revealLocation(uriStr: string, range: RevealRange): Promise<void> {
  const uri = vscode.Uri.parse(uriStr);
  const vsRange = new vscode.Range(
    new vscode.Position(range.start.line, range.start.character),
    new vscode.Position(range.end.line, range.end.character),
  );
  const doc = await vscode.workspace.openTextDocument(uri);
  const editor = await vscode.window.showTextDocument(doc, {
    viewColumn: vscode.ViewColumn.One,
    preserveFocus: false,
  });
  editor.selection = new vscode.Selection(vsRange.start, vsRange.start);
  editor.revealRange(vsRange, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
}

/** Open an http(s) URL a webview surfaced from doc-comment Markdown. Anything
 *  that isn't http/https is dropped — a doc comment must not be able to drive
 *  the host to a `command:`/`file:`/`vscode:` URI. `openExternal` itself shows
 *  the editor's own "an extension wants to open this URL" confirmation. */
export async function openExternalHref(href: string): Promise<void> {
  let parsed: vscode.Uri;
  try {
    parsed = vscode.Uri.parse(href, true);
  } catch {
    return;
  }
  if (parsed.scheme !== "http" && parsed.scheme !== "https") return;
  await vscode.env.openExternal(parsed);
}

/** Build the CSP-locked HTML shell for a Bynk webview: the payload embedded as
 *  a JSON global (`< ` escaped so a `</script>`-shaped string in it can't break
 *  out), then the vendored renderer bundle. `default-src 'none'` with a
 *  nonce-gated script is the same posture #846 established. */
export function renderWebviewHtml(opts: {
  webview: vscode.Webview;
  scriptUri: vscode.Uri;
  nonce: string;
  title: string;
  /** The `window.<globalName>` the bundle reads its payload from. */
  globalName: string;
  payload: unknown;
  /** #2093: the live view state (`window.__BYNK_VIEW__`) — the target the
   *  page persists for restore, and whether it draws a pin toggle. */
  view?: unknown;
  /** Extra CSS for the page body (beyond the shared base). */
  bodyCss?: string;
}): string {
  const { webview, scriptUri, nonce, title, globalName, payload, view, bodyCss } = opts;
  const payloadJson = JSON.stringify(payload).replace(/</g, "\\u003c");
  const viewJson = JSON.stringify(view ?? null).replace(/</g, "\\u003c");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src ${webview.cspSource} data:; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';">
<title>${title}</title>
<style>
  body { font-family: var(--vscode-font-family, sans-serif); color: var(--vscode-foreground); padding: 8px; }
  #root svg { max-width: 100%; }
  #bynk-toolbar { display: flex; justify-content: flex-end; gap: 6px; font-size: 0.85em; }
  #bynk-toolbar:empty { display: none; }
  #bynk-toolbar button { background: var(--vscode-button-secondaryBackground); color: var(--vscode-button-secondaryForeground); border: none; border-radius: 2px; padding: 2px 8px; cursor: pointer; }
  #bynk-toolbar button:hover { background: var(--vscode-button-secondaryHoverBackground); }
  #bynk-toolbar button[aria-pressed="true"] { background: var(--vscode-button-background); color: var(--vscode-button-foreground); }
${bodyCss ?? ""}
</style>
</head>
<body>
<div id="bynk-toolbar"></div>
<div id="root">Loading…</div>
<script nonce="${nonce}">window.${globalName} = ${payloadJson}; window.__BYNK_VIEW__ = ${viewJson};</script>
<script nonce="${nonce}" src="${scriptUri.toString()}"></script>
</body>
</html>`;
}
