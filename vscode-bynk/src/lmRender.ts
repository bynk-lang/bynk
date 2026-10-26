// #2463: the pure core of the language-model tools (`lmTools.ts`,
// `lmToolsCore.ts`) — each tool's model rendered as compact Markdown for a
// chat agent to read. No `vscode` import, so the unit tests compile against
// it directly (see tsconfig.test.json).
//
// DECISION C: models read prose and short lists better than raw JSON, so every
// renderer emits Markdown with `path:line:col` locations (workspace-relative,
// 1-based, as an agent quotes them back), and every result is capped
// (`capText`) with a note saying how to narrow the call.

import type { Explanation } from "./webview/explainTypes";
import type { DocModel } from "./webview/docTypes";
import type { ArchLoc, ArchModel } from "./webview/architectureTypes";
import type { WcBody, WcEnvelope, WcModel, WcRef } from "./webview/wireContractTypes";

/** A result's default ceiling, in characters (about 3,000 tokens). */
export const DEFAULT_MAX_CHARS = 12_000;

/** A 0-based LSP range's start. */
interface RangeLike {
  start: { line: number; character: number };
}

/** Turns a document URI into the path an agent sees (workspace-relative). */
export type RelPath = (uri: string) => string;

/** `path:line:col`, 1-based. */
export function fmtLoc(path: string, range: RangeLike): string {
  return `${path}:${range.start.line + 1}:${range.start.character + 1}`;
}

/** Cut `text` to `max` characters at a line boundary, saying so and how to
 *  ask for less. */
export function capText(text: string, max = DEFAULT_MAX_CHARS, narrower?: string): string {
  if (text.length <= max) return text;
  const cut = text.lastIndexOf("\n", max);
  const kept = text.slice(0, cut > 0 ? cut : max);
  const hint = narrower ? ` ${narrower}` : "";
  return `${kept}\n\n…(truncated: ${text.length - kept.length} more characters.${hint})`;
}

/** `bynk_explain_diagnostic`: the toolchain's own explanation of a code. */
export function renderExplanation(e: Explanation): string {
  return `# \`${e.title}\`\n\n${e.markdown.trim()}\n\nBook: ${e.href}\n`;
}

/** One diagnostic, as `bynk_get_diagnostics` and `bynk_check` list it. */
export interface DiagnosticLine {
  path: string;
  /** 1-based. */
  line: number;
  /** 1-based. */
  column: number;
  severity: "error" | "warning" | "info" | "hint";
  code: string | undefined;
  message: string;
}

/** A diagnostics list, errors first, then by path and position. */
export function renderDiagnostics(list: readonly DiagnosticLine[], scope: string): string {
  if (list.length === 0) return `No Bynk diagnostics ${scope}.\n`;
  const rank = { error: 0, warning: 1, info: 2, hint: 3 } as const;
  const sorted = [...list].sort(
    (a, b) =>
      rank[a.severity] - rank[b.severity] ||
      a.path.localeCompare(b.path) ||
      a.line - b.line ||
      a.column - b.column,
  );
  const errors = list.filter((d) => d.severity === "error").length;
  const warnings = list.filter((d) => d.severity === "warning").length;
  const head = `${errors} error(s), ${warnings} warning(s) ${scope}.`;
  const lines = sorted.map((d) => {
    const code = d.code ? `[${d.code}]` : "";
    return `- ${d.path}:${d.line}:${d.column} ${d.severity}${code}: ${d.message}`;
  });
  const codes = [...new Set(list.map((d) => d.code).filter((c): c is string => !!c))];
  const tip =
    codes.length > 0
      ? `\nExplain a code with \`bynk_explain_diagnostic\` (e.g. \`${codes[0]}\`).`
      : "";
  return `${head}\n\n${lines.join("\n")}\n${tip}\n`;
}

/** `bynk_check`: why a run that reported no diagnostic still didn't pass, or
 *  `undefined` when its diagnostics (or their absence) are the whole story. A
 *  non-zero exit with nothing parsed — a malformed `bynk.toml`, a crash, a
 *  `bynk` too old for `--format short`, a signal — must never read as a clean
 *  project, so this says so and quotes the tail of what the run printed. */
export function checkFailure(
  code: number | null,
  parsed: number,
  output: string,
  maxOutput = 2_000,
): string | undefined {
  if (code === 0 || parsed > 0) return undefined;
  const how = code === null ? "was stopped by a signal" : `exited with code ${code}`;
  const text = output.trim();
  const tail = text.length > maxOutput ? `…${text.slice(-maxOutput)}` : text;
  const shown = tail ? `\n\nIts output:\n\n\`\`\`text\n${tail}\n\`\`\`` : " It printed nothing.";
  return (
    `\`bynk check\` ${how} without reporting a diagnostic, so the project did not pass ` +
    `and its state is unknown.${shown}\n`
  );
}

/** `bynk_get_documentation`: every declaration of a unit, with its signature
 *  and doc comment. */
export function renderDocumentation(model: DocModel, rel: RelPath): string {
  const out = [`# ${model.unitKind} \`${model.unitName}\``, ""];
  if (model.unitDoc) out.push(model.unitDoc.trim(), "");
  for (const entry of model.entries) {
    const hashes = "#".repeat(Math.min(2 + entry.depth, 6));
    out.push(`${hashes} ${entry.kind} \`${entry.name}\` (${fmtLoc(rel(entry.uri), entry.range)})`);
    out.push("", entry.markdown.trim());
    if (!entry.documented) out.push("", "_(no doc comment)_");
    out.push("");
  }
  return `${out.join("\n").trimEnd()}\n`;
}

/** `bynk_get_architecture`: the project's units, what each declares, and the
 *  `consumes` edges between them. */
export function renderArchitecture(model: ArchModel, rel: RelPath): string {
  const at = (loc: ArchLoc) => fmtLoc(rel(loc.uri), loc.range);
  const out = [
    `# Architecture: ${model.nodes.length} unit(s), ${model.edges.length} dependency edge(s)`,
    "",
  ];
  for (const node of [...model.nodes].sort((a, b) => a.name.localeCompare(b.name))) {
    out.push(`## ${node.kind.toLowerCase()} \`${node.name}\` (${at(node.loc)})`);
    for (const s of node.services) {
      out.push(`- service \`${s.name}\`: ${s.handlerCount} handler(s) (${at(s.loc)})`);
    }
    for (const a of node.agents) {
      out.push(`- agent \`${a.name}\`: ${a.handlerCount} handler(s) (${at(a.loc)})`);
    }
    for (const c of node.capabilities) {
      const from = c.local ? "declared here" : `from \`${c.from ?? "?"}\``;
      out.push(`- capability \`${c.name}\` (${from})`);
    }
    for (const p of node.providers) {
      const ext = p.external ? ", external" : "";
      out.push(`- provides \`${p.capability}\` as \`${p.providerName}\`${ext}`);
    }
    const uses = model.edges.filter((e) => e.from === node.name);
    for (const e of uses) {
      const what = e.capabilities.length > 0 ? ` { ${e.capabilities.join(", ")} }` : "";
      out.push(`- consumes \`${e.to}\`${what}`);
    }
    out.push("");
  }
  return `${out.join("\n").trimEnd()}\n`;
}

function refText(r: WcRef): string {
  switch (r.kind) {
    case "Base":
      return r.base;
    case "Bytes":
      return "Bytes";
    case "Named":
      return r.name;
    case "Inst":
      return r.key;
    case "Unit":
      return "()";
    case "Unchecked":
      return `${r.reason} (not checked at the boundary)`;
  }
}

function envelopeText(env: WcEnvelope): string {
  switch (env.kind) {
    case "Empty":
      return "no parameters";
    case "Bare":
      return `\`${env.param}: ${refText(env.shape)}\``;
    case "Keyed":
      return env.params.map((p) => `\`${p.name}: ${refText(p.shape)}\``).join(", ");
    case "Raw":
      return `\`${env.param}\`, raw ${env.contentType}`;
  }
}

function bodyText(body: WcBody): string[] {
  switch (body.kind) {
    case "Scalar": {
      const preds = body.predicates.length > 0 ? ` where ${body.predicates.join(" && ")}` : "";
      const opaque = body.opaque ? "opaque " : "";
      return [`${opaque}${body.base}${preds}, sent as JSON ${body.json.toLowerCase()}`];
    }
    case "Record":
      return body.fields.map((f) => {
        const def = f.default !== null ? ` = ${f.default}` : "";
        return `  - \`${f.name}\`: ${refText(f.shape)}${def}`;
      });
    case "Sum":
      return [
        `sum, discriminated by \`${body.wireDiscriminant}\`:`,
        ...body.variants.map((v) => {
          const payload = v.payload.map((f) => `${f.name}: ${refText(f.shape)}`).join(", ");
          return `  - \`${v.name}\`${payload ? `(${payload})` : ""}`;
        }),
      ];
  }
}

/** `bynk_get_wire_contract`: what a handler accepts and answers on the wire. */
export function renderWireContract(model: WcModel, uri: string, rel: RelPath): string {
  const kind = model.kind;
  const route =
    kind.kind === "Http"
      ? `${kind.method} ${kind.path}`
      : kind.kind === "Cron"
        ? `cron "${kind.expr}"`
        : kind.kind.toLowerCase();
  const out = [
    `# \`${model.unit}\` · service \`${model.service}\` · ${route} (${fmtLoc(rel(uri), model.range)})`,
    "",
    `Request: ${envelopeText(model.envelope)}.`,
  ];
  if (model.responses.length > 0) {
    out.push("", "Responses:");
    for (const r of model.responses) {
      const why =
        r.origin.kind === "BoundaryImplicit"
          ? ` (added by the boundary: ${r.origin.why})`
          : r.origin.kind === "DeclaredSuccess"
            ? " (declared success)"
            : "";
      out.push(`- ${r.status} \`${r.variant}\`${why}`);
    }
  }
  if (model.contract) {
    out.push("", `Cross-context contract hash: \`${model.contract.hash}\`.`);
  }
  if (model.types.length > 0) {
    out.push("", "Boundary types:");
    for (const t of model.types) {
      const where = t.loc ? ` (${fmtLoc(rel(t.loc.uri), t.loc.range)})` : "";
      const owner = t.provenance.kind === "Consumed" ? `, from \`${t.provenance.ownerUnit}\`` : "";
      const lines = bodyText(t.body);
      // A record lists its fields beneath; a scalar or sum leads on the line.
      const lead = t.body.kind === "Record" ? "record" : lines.shift();
      out.push(`- \`${t.name}\`${owner}${where}: ${lead}`, ...lines);
    }
  }
  return `${out.join("\n").trimEnd()}\n`;
}

/** `bynk_describe_symbol`: what the language server says at a position —
 *  its hover (the type and doc) and where the symbol is defined. */
export function renderSymbol(
  at: string,
  hovers: readonly string[],
  definitions: readonly string[],
): string {
  if (hovers.length === 0 && definitions.length === 0) {
    return `Nothing is known at ${at} (not on a symbol, or the file has not been analysed).\n`;
  }
  const out = [`# Symbol at ${at}`, ""];
  if (hovers.length > 0) out.push(hovers.map((h) => h.trim()).join("\n\n---\n\n"), "");
  if (definitions.length > 0) out.push(`Defined at: ${definitions.join(", ")}`);
  return `${out.join("\n").trimEnd()}\n`;
}
