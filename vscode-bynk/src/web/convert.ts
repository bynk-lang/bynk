// `bynk-wasm` JSON → LSP shapes for the web extension's worker (#2102).
//
// The wire shapes are the ones the playground already consumes
// (`bynk_analyze` / `bynk_hover` / `bynk_complete`, bynk-wasm/src/lib.rs); this
// module only re-shapes them for LSP. Dependency-free (numeric LSP enum values,
// no `vscode` or LSP package imports) so the unit tests compile it directly.

import { LineIndex, type Position } from "./positions";

/** One `bynk_analyze` diagnostic (bynk-wasm `Diagnostic`). */
export interface WasmDiagnostic {
  from: number;
  to: number;
  severity: string;
  category: string;
  message: string;
  notes?: string[];
  labels?: string[];
}

/** One `bynk_complete` candidate (bynk-wasm `CompletionCandidate`). */
export interface WasmCompletion {
  label: string;
  kind: string;
  detail: string | null;
  insert_text: string | null;
}

export interface Range {
  start: Position;
  end: Position;
}

/** An LSP `Diagnostic`, restricted to the fields the worker sets. */
export interface LspDiagnostic {
  range: Range;
  severity: number;
  code: string;
  source: string;
  message: string;
}

/** An LSP `CompletionItem`, restricted to the fields the worker sets. */
export interface LspCompletionItem {
  label: string;
  kind: number;
  detail?: string;
  insertText?: string;
  insertTextFormat?: number;
}

// LSP `DiagnosticSeverity`.
const SEVERITY_ERROR = 1;
const SEVERITY_WARNING = 2;

// LSP `InsertTextFormat.Snippet`.
const INSERT_TEXT_SNIPPET = 2;

/**
 * LSP `CompletionItemKind` per `bynk_complete` kind. The same table as
 * bynk-lsp's `to_completion_item`, plus `local` (bynk-lsp emits locals as
 * `VARIABLE` from its own handler), so web and desktop show the same icons.
 */
export const COMPLETION_KIND: Record<string, number> = {
  unit: 9, // Module
  capability: 8, // Interface
  type: 22, // Struct
  keyword: 14, // Keyword
  snippet: 15, // Snippet
  variant: 20, // EnumMember
  member: 2, // Method
  field: 5, // Field
  constructor: 4, // Constructor
  function: 3, // Function
  constant: 21, // Constant
  local: 6, // Variable
};
const COMPLETION_KIND_TEXT = 1;

/**
 * Categories that, in single-file mode, mean "another unit isn't loaded"
 * rather than "this file is wrong" (DECISION B). The analysis sees one buffer,
 * so a `uses` of a sibling commons unit, or a suite's target, never resolves.
 */
const CROSS_UNIT_ROOTS = new Set(["bynk.uses.unknown_commons", "bynk.suite.unknown_target"]);

/**
 * Drop the diagnostics that single-file analysis can't get right (DECISION B):
 *
 * - `bynk.uses.unknown_commons` and `bynk.suite.unknown_target` always: the
 *   unit they name lives in another file the worker hasn't loaded.
 * - when either is present, every `bynk.resolve.*` diagnostic too: those are
 *   the names that import would have brought into scope, so they cascade from
 *   the missing unit. In a file with no cross-unit reference a `resolve`
 *   error is real and is kept.
 *
 * The status bar labels the mode "Bynk (web, single-file)" so the gap is
 * visible. Project loading (slice 2) removes the need for this filter.
 */
export function singleFileDiagnostics(diags: WasmDiagnostic[]): WasmDiagnostic[] {
  const crossUnit = diags.some((d) => CROSS_UNIT_ROOTS.has(d.category));
  return diags.filter(
    (d) =>
      !CROSS_UNIT_ROOTS.has(d.category) && !(crossUnit && d.category.startsWith("bynk.resolve.")),
  );
}

/** The message shown for a diagnostic: the message, then its notes and labels. */
function messageOf(d: WasmDiagnostic): string {
  const extra = [...(d.labels ?? []), ...(d.notes ?? [])].filter((s) => s.length > 0);
  return extra.length > 0 ? `${d.message}\n${extra.join("\n")}` : d.message;
}

/**
 * Convert `bynk_analyze` diagnostics to LSP diagnostics against `text`. Spans
 * come from the byte `from`/`to` (never the 1-indexed `line`/`col`, which count
 * bytes, not UTF-16 units).
 */
export function toLspDiagnostics(text: string, diags: WasmDiagnostic[]): LspDiagnostic[] {
  const index = new LineIndex(text);
  return diags.map((d) => {
    const from = Math.max(0, d.from);
    const to = Math.max(from, d.to);
    return {
      range: { start: index.positionOf(from), end: index.positionOf(to) },
      severity: d.severity === "error" ? SEVERITY_ERROR : SEVERITY_WARNING,
      code: d.category,
      source: "bynk",
      message: messageOf(d),
    };
  });
}

/** Hover Markdown for a `bynk_hover` type, or `undefined` for no hover. */
export function hoverMarkdown(ty: string | null | undefined): string | undefined {
  if (!ty) return undefined;
  return "```bynk\n" + ty + "\n```";
}

/** Convert `bynk_complete` candidates to LSP completion items. */
export function toLspCompletionItems(items: WasmCompletion[]): LspCompletionItem[] {
  return items.map((c) => {
    const item: LspCompletionItem = {
      label: c.label,
      kind: COMPLETION_KIND[c.kind] ?? COMPLETION_KIND_TEXT,
    };
    if (c.detail) item.detail = c.detail;
    // Snippet items carry `${n:…}` tab stops (as in bynk-lsp); everything else
    // inserts its label.
    if (c.insert_text) {
      item.insertText = c.insert_text;
      item.insertTextFormat = INSERT_TEXT_SNIPPET;
    }
    return item;
  });
}
