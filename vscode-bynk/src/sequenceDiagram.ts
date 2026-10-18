// #846: "Bynk: Show Sequence Diagram" — the extension's first webview panel.
//
// Issues the `bynk/sequenceModel` custom LSP request (this server's first
// non-standard request — see `bynk-lsp/src/sequence_request.rs`) for the
// handler at the invoking CodeLens/command position, renders the result as
// a Mermaid sequence diagram, and handles click-to-code: the webview posts
// plain JSON `{uri, range}` (never real VS Code instances — a webview has
// no access to the `vscode` module), hydrated here into a real
// `vscode.Uri`/`Range` and revealed — the same hydration posture
// `codelens.ts` uses for `editor.action.showReferences`, just travelling
// the opposite direction (webview -> host, not server -> client).

// #2093: the panel is now a `LivePanel`: it refreshes on edits, follows the
// cursor from handler to handler (unless pinned), and survives a reload.

import * as vscode from "vscode";
import type { LanguageClient } from "vscode-languageclient/node";

import { commandTarget, LivePanel, type ViewTarget } from "./livePanel";
import type { ViewRefresh } from "./viewRefresh";
import type { SequenceModel, SequencePayload } from "./webview/types";

interface SequenceModelParams {
  textDocument: { uri: string };
  position: { line: number; character: number };
}

export function registerSequenceDiagram(
  context: vscode.ExtensionContext,
  hub: ViewRefresh,
): LivePanel<SequencePayload> {
  const view = new LivePanel<SequencePayload>(context, {
    viewType: "bynkSequenceDiagram",
    commandLabel: "Show Sequence Diagram",
    script: "webview.js",
    globalName: "__BYNK_SEQUENCE_MODEL__",
    followsCursor: true,
    title: () => "Bynk Sequence Diagram",
    request: requestSequence,
    missMessage: "Bynk: no handler found at the cursor.",
  }, hub);
  context.subscriptions.push(
    view,
    vscode.commands.registerCommand(
      "bynk.showSequenceDiagram",
      (uriArg?: unknown, positionArg?: unknown) => {
        const target = commandTarget(uriArg, positionArg, true);
        if (!target) {
          void vscode.window.showErrorMessage(
            "Bynk: Show Sequence Diagram needs a .bynk file open (or a Show Sequence lens).",
          );
          return;
        }
        return view.show(target);
      },
    ),
  );
  return view;
}

async function requestSequence(
  client: LanguageClient,
  target: ViewTarget,
): Promise<SequencePayload | null> {
  if (!target.position) return null;
  const params: SequenceModelParams = {
    textDocument: { uri: target.uri },
    position: target.position,
  };
  const model = await client.sendRequest<SequenceModel | null>("bynk/sequenceModel", params);
  return model ? { model, uri: target.uri } : null;
}
