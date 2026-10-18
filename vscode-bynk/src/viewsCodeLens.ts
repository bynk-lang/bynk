// #2093 (DECISION D): an opt-in handler CodeLens, behind `bynk.viewsCodeLens`
// (off by default: handlers already carry the server's lenses). The proposal
// asks for `Sequence | Wire contract`; the server already puts a
// `Show Sequence` lens above every handler (#846, `bynk-lsp/src/lib.rs`
// `code_lens`), so this adds the missing half, a `Wire contract` lens beside
// it, rather than a second sequence lens. Client-side, like `testCodeLens.ts`;
// anchors come from the server's document symbols, where a handler is a
// `Method` child of its service/agent `Class`.

import * as vscode from "vscode";

import { handlerSymbols } from "./refreshScheduler";
import type { ViewRefresh } from "./viewRefresh";

function enabled(): boolean {
  return vscode.workspace.getConfiguration("bynk").get<boolean>("viewsCodeLens", false);
}

class ViewsCodeLensProvider implements vscode.CodeLensProvider, vscode.Disposable {
  private readonly emitter = new vscode.EventEmitter<void>();
  readonly onDidChangeCodeLenses = this.emitter.event;
  private readonly sub: vscode.Disposable;

  constructor(private readonly hub: ViewRefresh) {
    this.sub = vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("bynk.viewsCodeLens")) this.emitter.fire();
    });
  }

  async provideCodeLenses(document: vscode.TextDocument): Promise<vscode.CodeLens[]> {
    if (!enabled()) return [];
    const uri = document.uri.toString();
    return handlerSymbols(await this.hub.documentSymbols(document)).map((h) => {
      const start = h.symbol.selectionRange.start;
      const range = new vscode.Range(start.line, start.character, start.line, start.character);
      return new vscode.CodeLens(range, {
        title: "Wire contract",
        tooltip: "Show this handler's wire contract",
        command: "bynk.showWireContract",
        arguments: [uri, { line: start.line, character: start.character }],
      });
    });
  }

  dispose(): void {
    this.sub.dispose();
    this.emitter.dispose();
  }
}

export function registerViewsCodeLens(context: vscode.ExtensionContext, hub: ViewRefresh): void {
  const provider = new ViewsCodeLensProvider(hub);
  context.subscriptions.push(
    provider,
    vscode.languages.registerCodeLensProvider({ language: "bynk" }, provider),
  );
}
