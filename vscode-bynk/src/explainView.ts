// #2098: "Bynk: Explain Diagnostic Code…" and the "Explain …" quick fix — the
// offline, version-matched explanation of a diagnostic code, in a reusable
// "Bynk: Explanation" webview beside the editor.
//
// The text comes from the language server's `bynk/explain` custom request,
// which reads the installed toolchain's own compiled-in table, so it matches
// the compiler that emitted the diagnostic and needs no network (the
// diagnostic's `codeDescription` link still opens the website). The server
// offers the quick fix only to a client that advertises
// `experimental.explainDiagnosticCommand` (see `explainClientFeature`), and
// the action invokes the command below with the code as its argument. Invoked
// from the palette with no argument, the command asks for a code: one of the
// Bynk codes currently in the Problems panel, or one typed in (say, from CI
// output).

import * as vscode from "vscode";
import type { LanguageClient, StaticFeature } from "vscode-languageclient/node";

import { getNonce, openExternalHref, renderWebviewHtml, type WebviewMessage } from "./webviewHost";
import type { ExplainPayload, Explanation } from "./webview/explainTypes";

/** The command id; the server's code action names it (`EXPLAIN_COMMAND`). */
export const EXPLAIN_COMMAND = "bynk.explainDiagnostic";

/** Where the full code index lives, offered when a code has no explanation. */
const DIAGNOSTIC_INDEX = "https://bynk-lang.org/book/reference/diagnostics/";

const EXPLAIN_CSS = `
  h1.code { font-size: 1.2em; border-bottom: 1px solid var(--vscode-panel-border); padding-bottom: 8px; }
  h1.code code, .body code { font-family: var(--vscode-editor-font-family, monospace); }
  .body pre { background: var(--vscode-textCodeBlock-background); padding: 8px; overflow-x: auto; border-radius: 3px; }
  .more { margin-top: 16px; }
`;

let panel: vscode.WebviewPanel | undefined;

/** The client capability that tells the server this client runs
 *  `bynk.explainDiagnostic`, so it should offer the "Explain …" quick fix. */
export function explainClientFeature(): StaticFeature {
  return {
    fillClientCapabilities(capabilities) {
      capabilities.experimental = {
        ...(capabilities.experimental as Record<string, unknown> | undefined),
        explainDiagnosticCommand: true,
      };
    },
    initialize() {},
    getState: () => ({ kind: "static" }),
    clear() {},
  };
}

export function registerExplainView(
  context: vscode.ExtensionContext,
  getClient: () => LanguageClient | undefined,
): void {
  context.subscriptions.push(
    vscode.commands.registerCommand(EXPLAIN_COMMAND, (code?: unknown) =>
      explain(context, getClient, typeof code === "string" ? code : undefined),
    ),
  );
}

/** Run the command. Resolves to the explanation shown, `null` when the code
 *  has none, or `undefined` when nothing was asked (cancelled, no server). The
 *  return value is for tests; the command palette ignores it. */
async function explain(
  context: vscode.ExtensionContext,
  getClient: () => LanguageClient | undefined,
  given: string | undefined,
): Promise<Explanation | null | undefined> {
  const client = getClient();
  if (!client) {
    void vscode.window.showErrorMessage("Bynk: Explain needs the language server running.");
    return undefined;
  }
  const experimental = client.initializeResult?.capabilities.experimental as
    | Record<string, unknown>
    | undefined;
  if (experimental?.explain !== true) {
    void vscode.window.showErrorMessage(
      "Bynk: the running language server does not support in-editor explanations. " +
        "Update it, or run `bynk explain <code>` in a terminal.",
    );
    return undefined;
  }
  const code = (given ?? (await askForCode()))?.trim();
  if (!code) return undefined;

  let explanation: Explanation | null;
  try {
    explanation = await client.sendRequest<Explanation | null>("bynk/explain", { code });
  } catch (err) {
    void vscode.window.showErrorMessage(`Bynk: Explain failed: ${String(err)}`);
    return undefined;
  }
  if (!explanation) {
    void vscode.window
      .showInformationMessage(
        `Bynk: no extended explanation is written for \`${code}\` yet ` +
          "(or it is not a code this compiler emits).",
        "Open Diagnostic Index",
      )
      .then((pick) => {
        if (pick) void openExternalHref(DIAGNOSTIC_INDEX);
      });
    return null;
  }
  showPanel(context, { explanation });
  return explanation;
}

/** The distinct `bynk.*` codes of the diagnostics currently reported. */
export function reportedCodes(
  diagnostics: ReadonlyArray<readonly [vscode.Uri, readonly vscode.Diagnostic[]]>,
): string[] {
  const codes = new Set<string>();
  for (const [, list] of diagnostics) {
    for (const d of list) {
      const raw = typeof d.code === "object" && d.code !== null ? d.code.value : d.code;
      if (typeof raw === "string" && raw.startsWith("bynk.")) codes.add(raw);
    }
  }
  return [...codes].sort();
}

/** A quick pick of the codes in the Problems panel, or free text. */
async function askForCode(): Promise<string | undefined> {
  const codes = reportedCodes(vscode.languages.getDiagnostics());
  const typeOne = "Enter a diagnostic code…";
  if (codes.length > 0) {
    const pick = await vscode.window.showQuickPick([...codes, typeOne], {
      title: "Bynk: Explain Diagnostic Code",
      placeHolder: "A code reported in this workspace, or enter one",
    });
    if (pick !== typeOne) return pick;
  }
  return vscode.window.showInputBox({
    title: "Bynk: Explain Diagnostic Code",
    prompt: "A diagnostic code, e.g. bynk.resolve.unknown_type",
    placeHolder: "bynk.resolve.unknown_type",
  });
}

function showPanel(context: vscode.ExtensionContext, payload: ExplainPayload): void {
  if (!panel) {
    panel = vscode.window.createWebviewPanel(
      "bynkExplanation",
      "Bynk: Explanation",
      { viewColumn: vscode.ViewColumn.Beside, preserveFocus: true },
      {
        enableScripts: true,
        localResourceRoots: [vscode.Uri.joinPath(context.extensionUri, "out")],
      },
    );
    panel.onDidDispose(() => {
      panel = undefined;
    });
    panel.webview.onDidReceiveMessage((message: WebviewMessage) => {
      if (message?.type === "openExternal") void openExternalHref(message.href);
    });
  }
  panel.title = `Bynk: Explanation — ${payload.explanation.title}`;
  panel.webview.html = renderWebviewHtml({
    webview: panel.webview,
    scriptUri: panel.webview.asWebviewUri(
      vscode.Uri.joinPath(context.extensionUri, "out", "explainview.js"),
    ),
    nonce: getNonce(),
    title: "Bynk: Explanation",
    globalName: "__BYNK_EXPLAIN__",
    payload,
    bodyCss: EXPLAIN_CSS,
  });
  panel.reveal(vscode.ViewColumn.Beside, true);
}
