// #2470: "Bynk: Explore Refined Type" — the refined-type explorer panel.
//
// The hover on a refined type carries a "Try values…" link to the command
// below (offered only to a client that advertises
// `experimental.refinedTypeExplorerCommand`, see `refinedExplorerClientFeature`;
// the hover middleware in `extension.ts` trusts that one command, since a
// server's Markdown cannot run commands otherwise). The command asks the server
// for `bynk/refinedTypeModel` — the base, the clauses as the emitter lowers
// them, and a seeded list of examples — and opens a panel. The panel posts each
// candidate back here, where `refinedEval.ts` checks it.

import * as vscode from "vscode";
import type { LanguageClient, StaticFeature } from "vscode-languageclient/node";

import { checkCandidate } from "./refinedEval";
import { commandTarget } from "./livePanel";
import { getNonce, renderWebviewHtml } from "./webviewHost";
import type { CheckMessage, RefinedModel, RefinedPayload } from "./webview/refinedTypes";

/** The command id; the hover link names it (`EXPLORE_COMMAND`). */
export const EXPLORE_COMMAND = "bynk.exploreRefinedType";

const REFINED_CSS = `
  h1 { font-size: 1.2em; }
  h1 code, .rx-clauses code, pre.rx-decl { font-family: var(--vscode-editor-font-family, monospace); }
  .rx-kind { color: var(--vscode-descriptionForeground); font-weight: normal; font-size: 0.75em; text-transform: uppercase; letter-spacing: 0.05em; }
  h2 { font-size: 0.95em; text-transform: uppercase; letter-spacing: 0.03em; color: var(--vscode-descriptionForeground); margin-top: 20px; }
  pre.rx-decl { background: var(--vscode-textCodeBlock-background); padding: 8px; overflow-x: auto; border-radius: 3px; }
  .rx-chips { display: flex; flex-wrap: wrap; gap: 6px; }
  button.rx-chip { font-family: var(--vscode-editor-font-family, monospace); background: var(--vscode-badge-background); color: var(--vscode-badge-foreground); border: none; border-radius: 3px; padding: 2px 8px; cursor: pointer; }
  button.rx-chip:hover { outline: 1px solid var(--vscode-focusBorder); }
  .rx-note { color: var(--vscode-descriptionForeground); font-style: italic; }
  #rx-input { width: 100%; box-sizing: border-box; font-family: var(--vscode-editor-font-family, monospace); color: var(--vscode-input-foreground); background: var(--vscode-input-background); border: 1px solid var(--vscode-input-border, transparent); padding: 4px 6px; }
  .rx-verdict { margin-top: 8px; min-height: 1.4em; }
  .rx-ok { color: var(--vscode-testing-iconPassed, var(--vscode-charts-green)); }
  .rx-bad { color: var(--vscode-errorForeground); }
  .rx-verdict ul { margin: 4px 0 0; }
`;

let panel: vscode.WebviewPanel | undefined;
/** The model the open panel shows, for answering its checks. */
let current: RefinedModel | undefined;

/** The client capability that tells the server this client runs
 *  `bynk.exploreRefinedType`, so a refined type's hover may link to it. */
export function refinedExplorerClientFeature(): StaticFeature {
  return {
    fillClientCapabilities(capabilities) {
      capabilities.experimental = {
        ...(capabilities.experimental as Record<string, unknown> | undefined),
        refinedTypeExplorerCommand: true,
      };
    },
    initialize() {},
    getState: () => ({ kind: "static" }),
    clear() {},
  };
}

/** `provideHover` middleware: let the "Try values…" link run its one command. */
export function trustExplorerLink(hover: vscode.Hover | null | undefined): vscode.Hover | null | undefined {
  for (const c of hover?.contents ?? []) {
    if (c instanceof vscode.MarkdownString && c.value.includes(`command:${EXPLORE_COMMAND}?`)) {
      c.isTrusted = { enabledCommands: [EXPLORE_COMMAND] };
    }
  }
  return hover;
}

export function registerRefinedExplorer(
  context: vscode.ExtensionContext,
  getClient: () => LanguageClient | undefined,
): void {
  context.subscriptions.push(
    vscode.commands.registerCommand(EXPLORE_COMMAND, (uriArg?: unknown, positionArg?: unknown) =>
      explore(context, getClient, uriArg, positionArg),
    ),
  );
}

/** Run the command. Resolves to the model shown, or `undefined` when nothing
 *  was shown; the return value is for tests. */
async function explore(
  context: vscode.ExtensionContext,
  getClient: () => LanguageClient | undefined,
  uriArg: unknown,
  positionArg: unknown,
): Promise<RefinedModel | undefined> {
  const client = getClient();
  if (!client) {
    void vscode.window.showErrorMessage("Bynk: Explore Refined Type needs the language server running.");
    return undefined;
  }
  const target = commandTarget(uriArg, positionArg, true);
  if (!target?.position) {
    void vscode.window.showErrorMessage("Bynk: place the cursor on a refined type to explore it.");
    return undefined;
  }
  let model: RefinedModel | null;
  try {
    model = await client.sendRequest<RefinedModel | null>("bynk/refinedTypeModel", {
      textDocument: { uri: target.uri },
      position: target.position,
    });
  } catch (err) {
    void vscode.window.showErrorMessage(`Bynk: Explore Refined Type failed: ${String(err)}`);
    return undefined;
  }
  if (!model) {
    void vscode.window.showInformationMessage("Bynk: that is not a refined type with a predicate to explore.");
    return undefined;
  }
  showPanel(context, { model });
  return model;
}

function showPanel(context: vscode.ExtensionContext, payload: RefinedPayload): void {
  current = payload.model;
  if (!panel) {
    panel = vscode.window.createWebviewPanel(
      "bynkRefinedType",
      "Bynk: Refined Type",
      { viewColumn: vscode.ViewColumn.Beside, preserveFocus: false },
      {
        enableScripts: true,
        localResourceRoots: [vscode.Uri.joinPath(context.extensionUri, "out")],
      },
    );
    panel.onDidDispose(() => {
      panel = undefined;
      current = undefined;
    });
    panel.webview.onDidReceiveMessage((message: CheckMessage) => {
      if (message?.type !== "check" || typeof message.text !== "string" || !current) return;
      void panel?.webview.postMessage({
        type: "verdict",
        text: message.text,
        verdict: checkCandidate(current, message.text),
      });
    });
  }
  panel.title = `Bynk: Refined Type — ${payload.model.name}`;
  panel.webview.html = renderWebviewHtml({
    webview: panel.webview,
    scriptUri: panel.webview.asWebviewUri(
      vscode.Uri.joinPath(context.extensionUri, "out", "refinedExplorer.js"),
    ),
    nonce: getNonce(),
    title: "Bynk: Refined Type",
    globalName: "__BYNK_REFINED__",
    payload,
    bodyCss: REFINED_CSS,
  });
  panel.reveal(vscode.ViewColumn.Beside, false);
}
