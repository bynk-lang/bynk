// #855: "Bynk: Show Wire Contract" — the wire-contract-peek webview panel,
// the fourth consumer of the shared webview substrate (`webviewHost.ts`).
// Issues the `bynk/wireContract` custom LSP request (file-scoped + position,
// like #846's `bynk/sequenceModel` — see `bynk-lsp/src/wire_contract_request.rs`)
// for the handler at the cursor and renders the result as a JSON shape tree
// (no Mermaid, no markdown-it — see `webview/wire-render.ts`'s module doc).

// #2093: the panel is now a `LivePanel`: it refreshes on edits, follows the
// cursor from handler to handler (unless pinned), and survives a reload. The
// command also takes `(uri, position)` arguments, for the handler CodeLens and
// the project tree.

import * as vscode from "vscode";
import type { LanguageClient } from "vscode-languageclient/node";

import { commandTarget, LivePanel, type ViewTarget } from "./livePanel";
import type { ViewRefresh } from "./viewRefresh";
import type { WcModel, WcPayload } from "./webview/wireContractTypes";

interface WireContractParams {
  textDocument: { uri: string };
  position: { line: number; character: number };
}

// The panel's own styling, layered over the shared base in `renderWebviewHtml`
// (host-side, same posture as `documentationView.ts`'s `DOC_CSS` — the
// webview bundle itself injects no `<style>`, it only sets classes).
const WIRE_CSS = `
  h1 { display: flex; align-items: center; gap: 8px; cursor: pointer; font-family: var(--vscode-editor-font-family, monospace); }
  .wc-kind { color: var(--vscode-descriptionForeground); font-weight: normal; font-size: 0.75em; text-transform: uppercase; letter-spacing: 0.05em; }
  section.wc-card { border: 1px solid var(--vscode-panel-border); border-radius: 4px; padding: 8px 12px; margin: 12px 0; }
  section.wc-card > h2 { margin-top: 0; font-size: 0.95em; text-transform: uppercase; letter-spacing: 0.03em; color: var(--vscode-descriptionForeground); }
  .wc-muted { color: var(--vscode-descriptionForeground); font-style: italic; }
  .wc-hash { font-family: var(--vscode-editor-font-family, monospace); background: var(--vscode-textCodeBlock-background); padding: 2px 6px; border-radius: 3px; }
  code, pre { font-family: var(--vscode-editor-font-family, monospace); }
  pre { background: var(--vscode-textCodeBlock-background); padding: 8px; overflow-x: auto; border-radius: 3px; }
  table.wc-responses { border-collapse: collapse; width: 100%; }
  table.wc-responses th, table.wc-responses td { text-align: left; padding: 3px 8px; border-bottom: 1px solid var(--vscode-panel-border); }
  table.wc-responses tr.wc-row-muted { color: var(--vscode-descriptionForeground); font-style: italic; }
  table.wc-responses tr.wc-row-clickable { cursor: pointer; }
  a.type-chip { font-family: var(--vscode-editor-font-family, monospace); color: var(--vscode-textLink-foreground); cursor: pointer; text-decoration: none; }
  a.type-chip:hover { text-decoration: underline; }
  span.wc-badge { display: inline-block; font-family: var(--vscode-editor-font-family, monospace); background: var(--vscode-badge-background); color: var(--vscode-badge-foreground); border-radius: 3px; padding: 0 6px; margin: 0 4px 4px 0; font-size: 0.85em; }
  /* A StructuralOnly scalar's predicates: visibly distinct from a re-validated
     badge (outline, not filled) — see wire-render.ts's renderScalarBody doc on
     why an unqualified badge here would misstate the boundary check. */
  span.wc-badge-opaque { background: transparent; border: 1px dashed var(--vscode-descriptionForeground); color: var(--vscode-descriptionForeground); }
  details.wc-type { border: 1px solid var(--vscode-panel-border); border-radius: 4px; margin: 8px 0; padding: 4px 8px; }
  details.wc-type > summary { cursor: pointer; font-family: var(--vscode-editor-font-family, monospace); }
  .wc-shape { white-space: pre-wrap; }
`;

export function registerWireContract(
  context: vscode.ExtensionContext,
  hub: ViewRefresh,
): LivePanel<WcPayload> {
  const view = new LivePanel<WcPayload>(context, {
    viewType: "bynkWireContract",
    commandLabel: "Show Wire Contract",
    script: "wireContract.js",
    globalName: "__BYNK_WIRE_CONTRACT__",
    bodyCss: WIRE_CSS,
    followsCursor: true,
    title: (p) => (p ? `Bynk Wire Contract — ${p.model.service}` : "Bynk Wire Contract"),
    request: requestWireContract,
    missMessage: "Bynk: place the cursor on a service handler to see its wire contract.",
  }, hub);
  context.subscriptions.push(
    view,
    vscode.commands.registerCommand(
      "bynk.showWireContract",
      (uriArg?: unknown, positionArg?: unknown) => {
        const target = commandTarget(uriArg, positionArg, true);
        if (!target) {
          void vscode.window.showErrorMessage("Bynk: Show Wire Contract needs a .bynk file open.");
          return;
        }
        return view.show(target);
      },
    ),
  );
  return view;
}

async function requestWireContract(
  client: LanguageClient,
  target: ViewTarget,
): Promise<WcPayload | null> {
  if (!target.position) return null;
  const params: WireContractParams = {
    textDocument: { uri: target.uri },
    position: target.position,
  };
  const model = await client.sendRequest<WcModel | null>("bynk/wireContract", params);
  return model ? { model, uri: target.uri } : null;
}
