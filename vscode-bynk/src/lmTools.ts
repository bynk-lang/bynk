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
//
// DECISION B: `bynk_search_book` reads the Book from the installed toolchain
// (`bynk book`, embedded in the binary), so an answer matches the version the
// user runs; a `bynk` without the bundle falls back to the hosted
// `llms-full.txt`, and the result says the version may differ. It is
// desktop-only too: it needs the CLI, and the browser's fetch of the hosted
// Book is subject to the site's CORS policy.

import * as os from "node:os";

import * as vscode from "vscode";
import type { LanguageClient } from "vscode-languageclient/node";

import { requestArchitectureModel } from "./architectureMap";
import { BynkNotFoundError, findProjectRoot, runBynk } from "./cli";
import { diagnosticPath, parseShortDiagnostic } from "./cliCore";
import {
  capText,
  checkFailure,
  type BookSource,
  type DiagnosticLine,
  type ParsedBook,
  parseBook,
  renderArchitecture,
  renderBookSearch,
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

const HOSTED_BOOK = "https://bynk-lang.org/llms-full.txt";

/** How long a loaded Book is reused: a search per chat turn would otherwise
 *  re-read about 2 MB each time, and a toolchain upgrade shows up promptly. */
const BOOK_TTL_MS = 60_000;

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
    let code: number | null;
    try {
      // `bynk check` writes its diagnostics to stderr; read both streams.
      const run = await runBynk(["check", ".", "--format", "short"], root.fsPath, token);
      output = `${run.stdout}\n${run.stderr}`;
      code = run.code;
    } catch (e) {
      if (e instanceof BynkNotFoundError) throw new Error(e.message);
      throw e;
    }
    // A stopped run's partial output says nothing about the project.
    if (token.isCancellationRequested) throw new Error("`bynk check` was cancelled.");
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
    // Never report a failed run as a clean project.
    const failed = checkFailure(code, lines.length, output);
    if (failed) throw new Error(failed);
    // A project at a workspace folder's root has no shorter path to show.
    const where = relPath(root);
    const scope =
      where && where !== root.fsPath ? `from \`bynk check\` in ${where}` : "from `bynk check`";
    return textResult(capText(renderDiagnostics(lines, scope), maxChars(options)));
  }
}

interface LoadedBook {
  book: ParsedBook;
  source: BookSource;
}

/** The installed `bynk`'s version (`bynk 0.406.0` → `0.406.0`), if it runs. */
async function installedVersion(cwd: string, token: vscode.CancellationToken): Promise<string | undefined> {
  try {
    const run = await runBynk(["--version"], cwd, token);
    return /(\d+\.\d+\.\d+)/.exec(run.stdout)?.[1];
  } catch {
    return undefined;
  }
}

/** The Book: the installed toolchain's if its `bynk` carries one, else the
 *  hosted copy, labelled as such. */
async function loadBook(token: vscode.CancellationToken): Promise<LoadedBook> {
  const cwd = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? os.tmpdir();
  try {
    const run = await runBynk(["book"], cwd, token);
    const header = /^Bynk Book v(\d+\.\d+\.\d+)\n/.exec(run.stdout);
    if (run.code === 0 && header) {
      const book = parseBook(run.stdout);
      return { book, source: { kind: "installed", bookVersion: header[1], installedVersion: header[1] } };
    }
  } catch (e) {
    // No `bynk` to ask is worth saying; a `bynk` without `book` just falls back.
    if (!(e instanceof BynkNotFoundError)) throw e;
  }
  if (token.isCancellationRequested) throw new Error("The Book search was cancelled.");
  const installed = await installedVersion(cwd, token);
  const abort = new AbortController();
  const sub = token.onCancellationRequested(() => abort.abort());
  const timeout = setTimeout(() => abort.abort(), 20_000);
  try {
    const res = await fetch(HOSTED_BOOK, { signal: abort.signal });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const book = parseBook(await res.text());
    return { book, source: { kind: "hosted", bookVersion: book.version, installedVersion: installed } };
  } catch (e) {
    throw new Error(
      "The Bynk Book is unavailable: the installed `bynk` has no Book bundle (`bynk book`) and " +
        `${HOSTED_BOOK} could not be read (${e instanceof Error ? e.message : String(e)}). ` +
        "Update the Bynk toolchain, or read the Book at https://bynk-lang.org/book/.",
    );
  } finally {
    clearTimeout(timeout);
    sub.dispose();
  }
}

class SearchBookTool implements vscode.LanguageModelTool<{ query: string }> {
  private cached: { at: number; key: string; loaded: LoadedBook } | undefined;

  prepareInvocation(options: vscode.LanguageModelToolInvocationPrepareOptions<{ query: string }>) {
    return { invocationMessage: `Searching the Bynk Book for \`${options.input.query}\`` };
  }

  async invoke(
    options: vscode.LanguageModelToolInvocationOptions<{ query: string }>,
    token: vscode.CancellationToken,
  ) {
    const query = options.input.query?.trim();
    if (!query) throw new Error("Give a `query`: the words to look for in the Bynk Book.");
    // `bynk.bynkPath` picks the toolchain, so changing it must not reuse the old Book.
    const key = vscode.workspace.getConfiguration("bynk").get<string>("bynkPath", "");
    if (!this.cached || this.cached.key !== key || Date.now() - this.cached.at > BOOK_TTL_MS) {
      this.cached = { at: Date.now(), key, loaded: await loadBook(token) };
    }
    const { book, source } = this.cached.loaded;
    return textResult(capText(renderBookSearch(book, query, source), maxChars(options)));
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
    vscode.lm.registerTool("bynk_search_book", new SearchBookTool()),
  );
}
