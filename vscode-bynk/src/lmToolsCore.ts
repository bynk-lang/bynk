// #2463: the language-model tools that need only the VS Code API — the
// diagnostics the server has published and what it says about a symbol (hover
// + definition). No language-client or Node import, so the web extension
// registers these too (DECISION D); the desktop-only tools are in
// `lmTools.ts`. The rendering is in `lmRender.ts`.
//
// Positions in tool inputs and outputs are 1-based and paths are
// workspace-relative, as an agent reads and quotes them; LSP's 0-based
// positions stay inside this module.

import * as vscode from "vscode";

import {
  capText,
  DEFAULT_MAX_CHARS,
  type DiagnosticLine,
  renderDiagnostics,
  renderSymbol,
} from "./lmRender";

/** A tool's text result. */
export function textResult(text: string): vscode.LanguageModelToolResult {
  return new vscode.LanguageModelToolResult([new vscode.LanguageModelTextPart(text)]);
}

/** The character ceiling for one result: the caller's token budget when it
 *  gives one (at about three characters a token), never above the default. */
export function maxChars(options: { tokenizationOptions?: { tokenBudget: number } }): number {
  const budget = options.tokenizationOptions?.tokenBudget;
  return budget ? Math.min(DEFAULT_MAX_CHARS, Math.max(1_000, budget * 3)) : DEFAULT_MAX_CHARS;
}

/** The path an agent sees for a document URI: workspace-relative. */
export function relPath(uri: string | vscode.Uri): string {
  const u = typeof uri === "string" ? vscode.Uri.parse(uri) : uri;
  return vscode.workspace.asRelativePath(u, false);
}

/** Resolve a tool's `file` input — a URI, an absolute path, or a path relative
 *  to a workspace folder — to an existing document inside a workspace folder.
 *  The tools run without a confirmation, so an agent steered by text it read
 *  can't point one at a file or project outside the workspace. */
export async function resolveFile(file: string): Promise<vscode.Uri> {
  const candidates: vscode.Uri[] = [];
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(file)) {
    candidates.push(vscode.Uri.parse(file));
  } else if (file.startsWith("/") || /^[a-z]:[\\/]/i.test(file)) {
    candidates.push(vscode.Uri.file(file));
  } else {
    for (const folder of vscode.workspace.workspaceFolders ?? []) {
      candidates.push(vscode.Uri.joinPath(folder.uri, file));
    }
  }
  for (const uri of candidates) {
    if (!vscode.workspace.getWorkspaceFolder(uri)) continue;
    try {
      await vscode.workspace.fs.stat(uri);
      return uri;
    } catch {
      /* try the next */
    }
  }
  throw new Error(
    `No file \`${file}\` in this workspace. Give a path relative to the workspace folder, ` +
      "or an absolute path.",
  );
}

/** A 1-based `line` (and optional 1-based `column`, or a `name` on that line)
 *  as an LSP position. With neither, the line's first non-blank character. */
export function positionIn(
  doc: vscode.TextDocument,
  line: number,
  column?: number,
  name?: string,
): vscode.Position {
  if (!Number.isInteger(line) || line < 1 || line > doc.lineCount) {
    throw new Error(`Line ${line} is outside ${relPath(doc.uri)} (1–${doc.lineCount}).`);
  }
  const text = doc.lineAt(line - 1).text;
  if (column !== undefined) return new vscode.Position(line - 1, Math.max(0, column - 1));
  if (name) {
    const at = text.search(new RegExp(`\\b${escapeRegExp(name)}\\b`));
    if (at < 0) throw new Error(`\`${name}\` does not appear on line ${line} of ${relPath(doc.uri)}.`);
    return new vscode.Position(line - 1, at);
  }
  return new vscode.Position(line - 1, doc.lineAt(line - 1).firstNonWhitespaceCharacterIndex);
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** Whether a diagnostic belongs to Bynk: on a `.bynk` file or a `bynk.toml`,
 *  or carrying a `bynk.*` code. */
function isBynk(uri: vscode.Uri, d: vscode.Diagnostic): boolean {
  const code = codeOf(d);
  return /\.bynk$|(^|\/)bynk\.toml$/.test(uri.path) || (code?.startsWith("bynk.") ?? false);
}

function codeOf(d: vscode.Diagnostic): string | undefined {
  const raw = typeof d.code === "object" && d.code !== null ? d.code.value : d.code;
  return raw === undefined ? undefined : String(raw);
}

const SEVERITY: Record<vscode.DiagnosticSeverity, DiagnosticLine["severity"]> = {
  [vscode.DiagnosticSeverity.Error]: "error",
  [vscode.DiagnosticSeverity.Warning]: "warning",
  [vscode.DiagnosticSeverity.Information]: "info",
  [vscode.DiagnosticSeverity.Hint]: "hint",
};

interface DiagnosticsInput {
  file?: string;
}

/** `bynk_get_diagnostics`: what the language server currently reports. */
class DiagnosticsTool implements vscode.LanguageModelTool<DiagnosticsInput> {
  prepareInvocation(options: vscode.LanguageModelToolInvocationPrepareOptions<DiagnosticsInput>) {
    return {
      invocationMessage: options.input.file
        ? `Reading Bynk diagnostics for ${options.input.file}`
        : "Reading Bynk diagnostics",
    };
  }

  async invoke(options: vscode.LanguageModelToolInvocationOptions<DiagnosticsInput>) {
    const only = options.input.file ? await resolveFile(options.input.file) : undefined;
    const entries: ReadonlyArray<readonly [vscode.Uri, readonly vscode.Diagnostic[]]> = only
      ? [[only, vscode.languages.getDiagnostics(only)]]
      : vscode.languages.getDiagnostics();
    const lines: DiagnosticLine[] = [];
    for (const [uri, list] of entries) {
      for (const d of list) {
        if (!isBynk(uri, d)) continue;
        lines.push({
          path: relPath(uri),
          line: d.range.start.line + 1,
          column: d.range.start.character + 1,
          severity: SEVERITY[d.severity],
          code: codeOf(d),
          message: d.message,
        });
      }
    }
    const scope = only ? `in ${relPath(only)}` : "in the workspace";
    return textResult(
      capText(renderDiagnostics(lines, scope), maxChars(options), "Pass `file` to see one file."),
    );
  }
}

interface SymbolInput {
  file?: string;
  line?: number;
  column?: number;
  name?: string;
}

/** A hover's contents as Markdown. */
function hoverText(hover: vscode.Hover): string {
  return hover.contents
    .map((c) => {
      if (typeof c === "string") return c;
      if (c instanceof vscode.MarkdownString) return c.value;
      return `\`\`\`${c.language}\n${c.value}\n\`\`\``;
    })
    .join("\n\n");
}

/** Hover and definitions at one position, rendered. */
async function describeAt(uri: vscode.Uri, pos: vscode.Position): Promise<string> {
  const [hovers, defs] = await Promise.all([
    vscode.commands.executeCommand<vscode.Hover[]>("vscode.executeHoverProvider", uri, pos),
    vscode.commands.executeCommand<Array<vscode.Location | vscode.LocationLink>>(
      "vscode.executeDefinitionProvider",
      uri,
      pos,
    ),
  ]);
  const definitions = (defs ?? []).map((d) => {
    const [target, range] =
      "targetUri" in d ? [d.targetUri, d.targetSelectionRange ?? d.targetRange] : [d.uri, d.range];
    return `${relPath(target)}:${range.start.line + 1}`;
  });
  const at = `${relPath(uri)}:${pos.line + 1}:${pos.character + 1}`;
  return renderSymbol(at, (hovers ?? []).map(hoverText).filter((h) => h.trim()), definitions);
}

/** `bynk_describe_symbol`: the type, doc and definition of a symbol — at a
 *  position, or found by name across the workspace. */
class DescribeSymbolTool implements vscode.LanguageModelTool<SymbolInput> {
  prepareInvocation(options: vscode.LanguageModelToolInvocationPrepareOptions<SymbolInput>) {
    const { file, line, name } = options.input;
    const what = file && line ? `${file}:${line}` : `\`${name ?? "?"}\``;
    return { invocationMessage: `Describing Bynk symbol ${what}` };
  }

  async invoke(options: vscode.LanguageModelToolInvocationOptions<SymbolInput>) {
    const { file, line, column, name } = options.input;
    if (file && line !== undefined) {
      const uri = await resolveFile(file);
      const doc = await vscode.workspace.openTextDocument(uri);
      return textResult(
        capText(await describeAt(uri, positionIn(doc, line, column, name)), maxChars(options)),
      );
    }
    if (!name) {
      throw new Error("Give `name`, or `file` and `line` (with `column` or `name`).");
    }
    const found =
      (await vscode.commands.executeCommand<vscode.SymbolInformation[]>(
        "vscode.executeWorkspaceSymbolProvider",
        name,
      )) ?? [];
    // `file` without `line` narrows the search to that file.
    const only = file ? (await resolveFile(file)).toString() : undefined;
    const bynk = found.filter(
      (s) =>
        s.location.uri.path.endsWith(".bynk") && (!only || s.location.uri.toString() === only),
    );
    const exact = bynk.filter((s) => s.name === name);
    const picked = (exact.length > 0 ? exact : bynk).slice(0, 5);
    if (picked.length === 0) {
      const where = only ? relPath(only) : "the workspace";
      return textResult(`No Bynk symbol named \`${name}\` in ${where}.\n`);
    }
    const parts: string[] = [];
    for (const s of picked) {
      const doc = await vscode.workspace.openTextDocument(s.location.uri);
      // A symbol's range spans its declaration; hover on its name inside it.
      const start = doc.offsetAt(s.location.range.start);
      const body = doc.getText(s.location.range);
      const at = body.search(new RegExp(`\\b${escapeRegExp(s.name)}\\b`));
      const pos = doc.positionAt(start + Math.max(0, at));
      parts.push(await describeAt(s.location.uri, pos));
    }
    const more = (exact.length > 0 ? exact : bynk).length - picked.length;
    const tail = more > 0 ? `\n${more} more match(es) not shown; give \`file\` and \`line\`.\n` : "";
    return textResult(capText(parts.join("\n") + tail, maxChars(options)));
  }
}

/** Register the tools that work on desktop and on the web. */
export function registerCoreLmTools(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.lm.registerTool("bynk_get_diagnostics", new DiagnosticsTool()),
    vscode.lm.registerTool("bynk_describe_symbol", new DescribeSymbolTool()),
  );
}
