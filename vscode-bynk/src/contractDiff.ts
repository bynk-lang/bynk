// #2467: "Bynk: Show Contract Diff" — every boundary change since
// `bynk.schema.lock` (events and agent state, classified compatible or
// breaking by the same reconciliation `bynk check` runs) and every service
// contract whose hash differs from `bynk.deploy.lock`'s live record. Issues
// the project-scoped `bynk/contractDiff` request (see
// `bynk-lsp/src/contract_diff_request.rs`), like the architecture map, and
// is a `LivePanel`: it refreshes on edits and when either lock changes.

import * as vscode from "vscode";
import type { LanguageClient } from "vscode-languageclient/node";

import { commandTarget, LivePanel, type ViewTarget } from "./livePanel";
import { projectRootFor } from "./projectRoots";
import type { ViewRefresh } from "./viewRefresh";
import type { CdModel, CdPayload } from "./webview/contractDiffTypes";

const CONTRACT_DIFF_CSS = `
  h1 { font-family: var(--vscode-editor-font-family, monospace); }
  section.cd-card { border: 1px solid var(--vscode-panel-border); border-radius: 4px; padding: 8px 12px; margin: 12px 0; }
  section.cd-card > h2 { margin-top: 0; font-size: 0.95em; text-transform: uppercase; letter-spacing: 0.03em; color: var(--vscode-descriptionForeground); }
  .cd-muted { color: var(--vscode-descriptionForeground); font-style: italic; }
  table.cd-table { border-collapse: collapse; width: 100%; }
  table.cd-table td { text-align: left; padding: 3px 8px; border-bottom: 1px solid var(--vscode-panel-border); vertical-align: top; }
  tr.cd-clickable { cursor: pointer; }
  tr.cd-clickable:hover { background: var(--vscode-list-hoverBackground); }
  td.cd-class { text-transform: uppercase; font-size: 0.8em; letter-spacing: 0.04em; white-space: nowrap; }
  tr.cd-breaking td.cd-class { color: var(--vscode-editorWarning-foreground); font-weight: bold; }
  tr.cd-compatible td.cd-class { color: var(--vscode-testing-iconPassed, var(--vscode-descriptionForeground)); }
  td.cd-name { font-family: var(--vscode-editor-font-family, monospace); }
`;

interface ContractDiffParams {
  textDocument: { uri: string };
}

/** Fetch the contract diff for the project `uri` belongs to, through the hub's
 *  cache (`fresh` for an explicit command). */
export async function requestContractDiff(
  client: LanguageClient,
  hub: ViewRefresh,
  uri: string,
  fresh = false,
): Promise<CdModel | null> {
  const root = await projectRootFor(vscode.Uri.parse(uri));
  const params: ContractDiffParams = { textDocument: { uri } };
  return hub.cache.get(
    `bynk/contractDiff:${root?.toString() ?? uri}`,
    () => client.sendRequest<CdModel | null>("bynk/contractDiff", params),
    { fresh },
  );
}

export function registerContractDiff(
  context: vscode.ExtensionContext,
  hub: ViewRefresh,
): LivePanel<CdPayload> {
  const view = new LivePanel<CdPayload>(context, {
    viewType: "bynkContractDiff",
    commandLabel: "Show Contract Diff",
    script: "contractDiff.js",
    globalName: "__BYNK_CONTRACT_DIFF__",
    bodyCss: CONTRACT_DIFF_CSS,
    followsCursor: false,
    title: () => "Bynk Contract Diff",
    request: async (client: LanguageClient, target: ViewTarget, h: ViewRefresh, fresh: boolean) => {
      const model = await requestContractDiff(client, h, target.uri, fresh);
      return model ? { model, uri: target.uri } : null;
    },
    missMessage:
      "Bynk: no contract diff for this project (not a project file, or no committed analysis yet).",
  }, hub);
  context.subscriptions.push(
    view,
    vscode.commands.registerCommand("bynk.showContractDiff", (uriArg?: unknown) => {
      const target = commandTarget(uriArg, undefined, false);
      if (!target) {
        void vscode.window.showErrorMessage(
          "Bynk: Show Contract Diff needs a .bynk file open (to resolve the project).",
        );
        return;
      }
      return view.show(target);
    }),
  );
  return view;
}
