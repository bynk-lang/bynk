// #847: "Bynk: Show Documentation" — the documentation webview panel, the
// second consumer of the shared webview substrate (`webviewHost.ts`) #846
// introduced. Issues the `bynk/documentationModel` custom LSP request for the
// active file and renders every declaration as a reference page.
//
// Simpler than #846's sequence view in one way — the request carries no cursor
// position (the page is the whole file, Decision A) — and richer in another: it
// also handles an `openExternal` message, since rendered doc-comment Markdown
// can contain links (gated to http(s) by `webviewHost.openExternalHref`).

// #2093: the panel is now a `LivePanel`: it refreshes on edits to its file
// and survives a reload. The page is file-scoped, so it does not follow the
// cursor.

import * as vscode from "vscode";
import type { LanguageClient } from "vscode-languageclient/node";

import { commandTarget, LivePanel, type ViewTarget } from "./livePanel";
import type { ViewRefresh } from "./viewRefresh";
import type { DocModel, DocPayload } from "./webview/docTypes";

interface DocumentationModelParams {
  textDocument: { uri: string };
}

// The doc page's own styling, layered over the shared base in `renderWebviewHtml`.
const DOC_CSS = `
  header { border-bottom: 1px solid var(--vscode-panel-border); margin-bottom: 12px; padding-bottom: 8px; }
  .kind { color: var(--vscode-descriptionForeground); font-weight: normal; font-size: 0.75em; text-transform: uppercase; letter-spacing: 0.05em; margin-right: 8px; }
  .name { font-family: var(--vscode-editor-font-family, monospace); }
  h1, h2, h3, h4 { cursor: pointer; }
  .entry { border-top: 1px solid var(--vscode-panel-border); padding-top: 4px; }
  .body pre { background: var(--vscode-textCodeBlock-background); padding: 8px; overflow-x: auto; border-radius: 3px; }
  .body code { font-family: var(--vscode-editor-font-family, monospace); }
  .no-doc { color: var(--vscode-descriptionForeground); font-style: italic; }
  .toggle { display: inline-flex; align-items: center; gap: 4px; font-size: 0.85em; color: var(--vscode-descriptionForeground); }
  #root[data-hide-undocumented="true"] .entry[data-documented="false"] { display: none; }
`;

export function registerDocumentationView(
  context: vscode.ExtensionContext,
  hub: ViewRefresh,
): LivePanel<DocPayload> {
  const view = new LivePanel<DocPayload>(context, {
    viewType: "bynkDocumentation",
    commandLabel: "Show Documentation",
    script: "docview.js",
    globalName: "__BYNK_DOC_MODEL__",
    bodyCss: DOC_CSS,
    scope: "file",
    followsCursor: false,
    title: (p) => (p ? `Bynk Documentation — ${p.model.unitName}` : "Bynk Documentation"),
    request: requestDocumentation,
    missMessage:
      "Bynk: no documentation for this file (not a project file, or no committed analysis yet).",
  }, hub);
  context.subscriptions.push(
    view,
    vscode.commands.registerCommand("bynk.showDocumentation", (uriArg?: string | vscode.Uri) => {
      const target = commandTarget(uriArg, undefined, false);
      if (!target) {
        void vscode.window.showErrorMessage("Bynk: Show Documentation needs a .bynk file open.");
        return;
      }
      return view.show(target);
    }),
  );
  return view;
}

async function requestDocumentation(
  client: LanguageClient,
  target: ViewTarget,
): Promise<DocPayload | null> {
  const params: DocumentationModelParams = { textDocument: { uri: target.uri } };
  const model = await client.sendRequest<DocModel | null>("bynk/documentationModel", params);
  return model ? { model, uri: target.uri } : null;
}
