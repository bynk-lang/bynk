// #851: "Bynk: Show Architecture Map" — the whole-project architecture
// webview, the third consumer of the shared webview substrate (`webviewHost.ts`)
// #846 introduced and #847 extracted. Issues the `bynk/architectureModel`
// custom LSP request (project-scoped — see `bynk-lsp/src/architecture_request.rs`)
// for the active file's project and renders the result as a Mermaid `flowchart`.
//
// DECISION B (per-project, nearest `bynk.toml`): like the sequence/documentation
// views, this needs an active `.bynk` editor to resolve *which* project's round
// to read — the request's `textDocument` is that resolution key, not a scope
// restriction on the result (the model covers every context/adapter in that
// project, not just the active file).

// #2093: the panel is now a `LivePanel`: it refreshes on any `.bynk` or
// `bynk.toml` edit in the project, and survives a reload. The model fetch goes
// through the shared `ModelCache` (`requestArchitectureModel`), so another
// consumer of the same model shares one request per refresh.

import * as vscode from "vscode";
import type { LanguageClient } from "vscode-languageclient/node";

import { commandTarget, LivePanel, type ViewTarget } from "./livePanel";
import type { ViewRefresh } from "./viewRefresh";
import type { ArchModel, ArchPayload } from "./webview/architectureTypes";

interface ArchitectureModelParams {
  textDocument: { uri: string };
}

/** Fetch the architecture model for the project `uri` belongs to, through the
 *  hub's cache (`fresh` for an explicit command). */
export function requestArchitectureModel(
  client: LanguageClient,
  hub: ViewRefresh,
  uri: string,
  fresh = false,
): Promise<ArchModel | null> {
  const params: ArchitectureModelParams = { textDocument: { uri } };
  return hub.cache.get(
    `bynk/architectureModel:${uri}`,
    () => client.sendRequest<ArchModel | null>("bynk/architectureModel", params),
    { fresh },
  );
}

export function registerArchitectureMap(
  context: vscode.ExtensionContext,
  hub: ViewRefresh,
): LivePanel<ArchPayload> {
  const view = new LivePanel<ArchPayload>(context, {
    viewType: "bynkArchitectureMap",
    commandLabel: "Show Architecture Map",
    script: "architecture.js",
    globalName: "__BYNK_ARCH_MODEL__",
    scope: "project",
    followsCursor: false,
    title: () => "Bynk Architecture Map",
    request: async (client: LanguageClient, target: ViewTarget, h: ViewRefresh, fresh: boolean) => {
      const model = await requestArchitectureModel(client, h, target.uri, fresh);
      return model ? { model } : null;
    },
    missMessage:
      "Bynk: no architecture model for this project (not a project file, or no committed analysis yet).",
  }, hub);
  context.subscriptions.push(
    view,
    vscode.commands.registerCommand("bynk.showArchitectureMap", (uriArg?: string | vscode.Uri) => {
      const target = commandTarget(uriArg, undefined, false);
      if (!target) {
        void vscode.window.showErrorMessage(
          "Bynk: Show Architecture Map needs a .bynk file open (to resolve the project).",
        );
        return;
      }
      return view.show(target);
    }),
  );
  return view;
}
