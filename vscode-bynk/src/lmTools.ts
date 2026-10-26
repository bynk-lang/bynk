// #2463: language-model tools — the Bynk toolchain's answers, callable by any
// chat agent in VS Code (Copilot's agent mode, or another extension through
// `vscode.lm.invokeTool`). Bynk is young, so a model's training data on it is
// thin and dated; these let it ask the installed toolchain instead of
// guessing, and quote the same text a person sees in the editor.
//
// DECISION A: every tool is read-only, so none asks for confirmation. Each is
// a thin adapter over a request the extension already makes for a view —
// `bynk/explain` (#2098), `bynk/documentationModel` (#847),
// `bynk/architectureModel` (#851), `bynk/wireContract` (#855) — or over
// `bynk check`; the rendering is in `lmRender.ts`. These need the native
// language server or the CLI, so they are desktop-only (`!isWeb` in the
// manifest); `lmToolsCore.ts` has the two that also run in vscode.dev.

import * as vscode from "vscode";
import type { LanguageClient } from "vscode-languageclient/node";

import { requestArchitectureModel } from "./architectureMap";
import { BynkNotFoundError, findProjectRoot, runBynk } from "./cli";
import { diagnosticPath, parseShortDiagnostic } from "./cliCore";
import {
  capText,
  type DiagnosticLine,
  renderArchitecture,
  renderDiagnostics,
  renderDocumentation,
  renderExplanation,
  renderWireContract,
} from "./lmRender";
import { maxChars, positionIn, registerCoreLmTools, relPath, resolveFile, textResult } from "./lmToolsCore";
import { projectRootFor } from "./projectRoots";
import type { ViewRefresh } from "./viewRefresh";
import type { DocModel } from "./webview/docTypes";
import type { Explanation } from "./webview/explainTypes";
import type { WcModel } from "./webview/wireContractTypes";

const DIAGNOSTIC_INDEX = "https://bynk-lang.org/book/reference/diagnostics/";

type GetClient = () => LanguageClient | undefined;

/** The running client, if it serves the custom request `feature` advertises
 *  under `experimental`; otherwise an error the agent can relay. */
function clientFor(getClient: GetClient, feature: string): LanguageClient {
  const client = getClient();
  if (!client) {
    throw new Error("The Bynk language server is not running. Open a .bynk file, or run `Bynk: Restart Language Server`.");
  }
  const experimental = client.initializeResult?.capabilities.experimental as
    | Record<string, unknown>
    | undefined;
  if (experimental?.[feature] !== true) {
    throw new Error(`The running Bynk language server does not support \`${feature}\`; update it.`);
  }
  return client;
}

/** The `.bynk` file that picks a project: the given one, else the active
 *  editor's, else the first in the workspace. */
async function projectFile(file: string | undefined): Promise<vscode.Uri> {
  if (file) return resolveFile(file);
  const active = vscode.window.activeTextEditor?.document;
  if (active?.languageId === "bynk") return active.uri;
  const [first] = await vscode.workspace.findFiles("**/*.bynk", "**/{node_modules,.bynk}/**", 1);
  if (!first) throw new Error("There is no .bynk file in this workspace.");
  return first;
}

class ExplainTool implements vscode.LanguageModelTool<{ code: string }> {
  constructor(private readonly getClient: GetClient) {}

  prepareInvocation(options: vscode.LanguageModelToolInvocationPrepareOptions<{ code: string }>) {
    return { invocationMessage: `Explaining \`${options.input.code}\`` };
  }

  async invoke(options: vscode.LanguageModelToolInvocationOptions<{ code: string }>) {
    const client = clientFor(this.getClient, "explain");
    const code = options.input.code.trim();
    const e = await client.sendRequest<Explanation | null>("bynk/explain", { code });
    if (!e) {
      return textResult(
        `No extended explanation is written for \`${code}\` yet, or it is not a code this ` +
          `compiler emits. The full index of codes: ${DIAGNOSTIC_INDEX}\n`,
      );
    }
    return textResult(capText(renderExplanation(e), maxChars(options)));
  }
}

class DocumentationTool implements vscode.LanguageModelTool<{ file: string }> {
  constructor(private readonly getClient: GetClient) {}

  prepareInvocation(options: vscode.LanguageModelToolInvocationPrepareOptions<{ file: string }>) {
    return { invocationMessage: `Reading Bynk documentation for ${options.input.file}` };
  }

  async invoke(options: vscode.LanguageModelToolInvocationOptions<{ file: string }>) {
    const client = clientFor(this.getClient, "documentationModel");
    const uri = await resolveFile(options.input.file);
    const model = await client.sendRequest<DocModel | null>("bynk/documentationModel", {
      textDocument: { uri: uri.toString() },
    });
    if (!model) {
      return textResult(
        `No documentation for ${relPath(uri)}: it is not part of a Bynk project, or the ` +
          "project has not been analysed yet.\n",
      );
    }
    return textResult(capText(renderDocumentation(model, relPath), maxChars(options)));
  }
}

class ArchitectureTool implements vscode.LanguageModelTool<{ file?: string }> {
  constructor(
    private readonly getClient: GetClient,
    private readonly hub: ViewRefresh,
  ) {}

  prepareInvocation() {
    return { invocationMessage: "Reading the Bynk project's architecture" };
  }

  async invoke(options: vscode.LanguageModelToolInvocationOptions<{ file?: string }>) {
    const client = clientFor(this.getClient, "architectureModel");
    const uri = await projectFile(options.input.file);
    const model = await requestArchitectureModel(client, this.hub, uri.toString());
    if (!model) {
      return textResult(
        `No architecture for the project of ${relPath(uri)}: it is not part of a Bynk ` +
          "project, or the project has not been analysed yet.\n",
      );
    }
    return textResult(capText(renderArchitecture(model, relPath), maxChars(options)));
  }
}

interface WireContractInput {
  file: string;
  line: number;
  column?: number;
}

class WireContractTool implements vscode.LanguageModelTool<WireContractInput> {
  constructor(private readonly getClient: GetClient) {}

  prepareInvocation(options: vscode.LanguageModelToolInvocationPrepareOptions<WireContractInput>) {
    return {
      invocationMessage: `Reading the wire contract at ${options.input.file}:${options.input.line}`,
    };
  }

  async invoke(options: vscode.LanguageModelToolInvocationOptions<WireContractInput>) {
    const client = clientFor(this.getClient, "wireContract");
    const uri = await resolveFile(options.input.file);
    const doc = await vscode.workspace.openTextDocument(uri);
    const position = positionIn(doc, options.input.line, options.input.column);
    const model = await client.sendRequest<WcModel | null>("bynk/wireContract", {
      textDocument: { uri: uri.toString() },
      position: { line: position.line, character: position.character },
    });
    if (!model) {
      return textResult(
        `No service handler at ${relPath(uri)}:${options.input.line}. Give a line inside a ` +
          "service's `on …` handler.\n",
      );
    }
    return textResult(capText(renderWireContract(model, uri.toString(), relPath), maxChars(options)));
  }
}

class CheckTool implements vscode.LanguageModelTool<{ file?: string }> {
  prepareInvocation() {
    return { invocationMessage: "Running `bynk check`" };
  }

  async invoke(
    options: vscode.LanguageModelToolInvocationOptions<{ file?: string }>,
    token: vscode.CancellationToken,
  ) {
    const root = options.input.file
      ? await projectRootFor(await resolveFile(options.input.file))
      : await findProjectRoot();
    if (!root) throw new Error("No bynk.toml found: `bynk check` runs in a Bynk project.");
    let output: string;
    try {
      // `bynk check` writes its diagnostics to stderr; read both streams.
      const { stdout, stderr } = await runBynk(["check", ".", "--format", "short"], root.fsPath, token);
      output = `${stdout}\n${stderr}`;
    } catch (e) {
      if (e instanceof BynkNotFoundError) throw new Error(e.message);
      throw e;
    }
    const lines: DiagnosticLine[] = [];
    for (const raw of output.split(/\r?\n/)) {
      const d = parseShortDiagnostic(raw);
      if (!d) continue;
      lines.push({
        path: relPath(vscode.Uri.file(diagnosticPath(root.fsPath, d.file))),
        line: d.line,
        column: d.column,
        severity: d.severity,
        code: d.code,
        message: d.message,
      });
    }
    // A project at a workspace folder's root has no shorter path to show.
    const where = relPath(root);
    const scope =
      where && where !== root.fsPath ? `from \`bynk check\` in ${where}` : "from `bynk check`";
    return textResult(capText(renderDiagnostics(lines, scope), maxChars(options)));
  }
}

/** Register every Bynk language-model tool (desktop). */
export function registerLmTools(
  context: vscode.ExtensionContext,
  getClient: GetClient,
  hub: ViewRefresh,
): void {
  registerCoreLmTools(context);
  context.subscriptions.push(
    vscode.lm.registerTool("bynk_explain_diagnostic", new ExplainTool(getClient)),
    vscode.lm.registerTool("bynk_get_documentation", new DocumentationTool(getClient)),
    vscode.lm.registerTool("bynk_get_architecture", new ArchitectureTool(getClient, hub)),
    vscode.lm.registerTool("bynk_get_wire_contract", new WireContractTool(getClient)),
    vscode.lm.registerTool("bynk_check", new CheckTool()),
  );
}
