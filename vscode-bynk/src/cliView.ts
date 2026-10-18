// A host-rendered webview for the CLI-driven views (#2101 deploy plan, #2095
// doctor): the markup is built in the extension host from `bynk`'s JSON output
// by a pure, unit-tested renderer, so these views need no browser bundle of
// their own. It keeps the shared substrate's posture (`webviewHost.ts`): a
// strict CSP with a per-render nonce, and no `command:` URIs. Buttons carry a
// `data-action` (plus any `data-*` arguments), which a small inline script
// posts back as `{ type: "action", action, args }`.

import * as vscode from "vscode";

import { escapeHtml } from "./cliCore";
import { getNonce } from "./webviewHost";

/** A button press posted from an action view. */
export interface ActionMessage {
  type: "action";
  action: string;
  args: Record<string, string>;
}

export function isActionMessage(m: unknown): m is ActionMessage {
  return (
    !!m &&
    typeof m === "object" &&
    (m as ActionMessage).type === "action" &&
    typeof (m as ActionMessage).action === "string"
  );
}

/** The CSP-locked page for an action view: `bodyHtml` (already escaped by its
 *  renderer) plus the button wiring. */
export function renderActionHtml(webview: vscode.Webview, title: string, bodyHtml: string, css = ""): string {
  const nonce = getNonce();
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';">
<title>${escapeHtml(title)}</title>
<style>
  body { font-family: var(--vscode-font-family, sans-serif); color: var(--vscode-foreground); padding: 8px 16px; }
  code { font-family: var(--vscode-editor-font-family, monospace); }
${css}
</style>
</head>
<body>
${bodyHtml}
<script nonce="${nonce}">
  const vscode = acquireVsCodeApi();
  document.addEventListener("click", (e) => {
    const el = e.target instanceof Element ? e.target.closest("[data-action]") : null;
    if (!el) return;
    const args = {};
    for (const [k, v] of Object.entries(el.dataset)) if (k !== "action") args[k] = v;
    vscode.postMessage({ type: "action", action: el.dataset.action, args });
  });
</script>
</body>
</html>`;
}
