// #2196: "Bynk: Show Agent State" — the agent state view (agent-state-views
// track, slice 2). Issues `bynk/agentStateModel` for the agent at the
// command/lens position and renders its state machines (`webview/state.ts`).
//
// A `LivePanel` like the other model views (ADR 0504), but agent-scoped: it
// follows the cursor from agent to agent (`followScope: "owner"`), and inside
// one agent it only moves the highlight to the handler under the cursor,
// without re-requesting the model.

import * as vscode from "vscode";
import type { LanguageClient } from "vscode-languageclient/node";

import { commandTarget, LivePanel, type ViewTarget } from "./livePanel";
import type { ViewRefresh } from "./viewRefresh";
import type { AgentStateModel, AgentStatePayload } from "./webview/stateTypes";

interface AgentStateModelParams {
  textDocument: { uri: string };
  position: { line: number; character: number };
}

const BODY_CSS = `
  .controls { display: flex; flex-wrap: wrap; align-items: baseline; gap: 1em; margin-bottom: 0.5em; }
  .controls h2 { margin: 0; font-size: 1.1em; font-weight: 600; }
  .controls select { margin-left: 0.25em; }
  a { color: var(--vscode-textLink-foreground); cursor: pointer; }
  .diagram { overflow-x: auto; margin: 0.5em 0 1em; }
  .diagram .label.focus { font-weight: 700; text-decoration: underline; }
  table.states { border-collapse: collapse; font-family: var(--vscode-editor-font-family); font-size: 0.95em; }
  table.states th, table.states td { border: 1px solid var(--vscode-panel-border, #555); padding: 0.3em 0.6em; text-align: left; vertical-align: top; }
  table.states thead th { background: var(--vscode-editor-inactiveSelectionBackground, transparent); }
  table.states th.unreachable { opacity: 0.55; font-style: italic; }
  table.states .entry { cursor: pointer; white-space: nowrap; }
  table.states .entry.moves { font-weight: 600; }
  table.states .muted { color: var(--vscode-descriptionForeground); }
  table.states tr.focus { outline: 2px solid var(--vscode-focusBorder); outline-offset: -2px; }
`;

export function registerAgentStateView(
  context: vscode.ExtensionContext,
  hub: ViewRefresh,
): LivePanel<AgentStatePayload> {
  const view = new LivePanel<AgentStatePayload>(context, {
    viewType: "bynkAgentState",
    commandLabel: "Show Agent State",
    script: "state.js",
    globalName: "__BYNK_AGENT_STATE__",
    bodyCss: BODY_CSS,
    followsCursor: true,
    followScope: "owner",
    scopeNoun: "agent",
    title: (p) => (p ? `Bynk State: ${p.model.agent}` : "Bynk Agent State"),
    request: requestAgentState,
    missMessage: "Bynk: no agent found at the cursor.",
  }, hub);
  context.subscriptions.push(
    view,
    vscode.commands.registerCommand(
      "bynk.showAgentState",
      (uriArg?: unknown, positionArg?: unknown) => {
        const target = commandTarget(uriArg, positionArg, true);
        if (!target) {
          void vscode.window.showErrorMessage(
            "Bynk: Show Agent State needs a .bynk file open (or a Show State lens).",
          );
          return;
        }
        return view.show(target);
      },
    ),
  );
  return view;
}

async function requestAgentState(
  client: LanguageClient,
  target: ViewTarget,
): Promise<AgentStatePayload | null> {
  if (!target.position) return null;
  const params: AgentStateModelParams = {
    textDocument: { uri: target.uri },
    position: target.position,
  };
  const model = await client.sendRequest<AgentStateModel | null>("bynk/agentStateModel", params);
  return model ? { model, uri: target.uri } : null;
}
