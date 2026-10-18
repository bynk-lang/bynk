// `bynk-wasm` JSON → LSP shapes for the web extension's worker (#2102).
//
// The wire shapes are the ones the playground already consumes
// (`bynk_analyze` / `bynk_hover` / `bynk_complete`, bynk-wasm/src/lib.rs); this
// module only re-shapes them for LSP. Dependency-free (numeric LSP enum values,
// no `vscode` or LSP package imports) so the unit tests compile it directly.

import { LineIndex, type Position } from "./positions";
import type { WasmOutcome } from "./wasmCall";

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

// The single-file filter (DECISION B). The rules come from analysing every
// `bynkc/tests/fixtures/positive` source file on its own. Those files all
// check clean as projects, so whatever single-file analysis reports for them
// is a single-file artefact.

/**
 * Categories that mean "another unit isn't loaded", not "this file is wrong":
 * a `uses` of a sibling commons, a `consumes` of a sibling context, or a
 * suite's target. They are dropped, and they also mark the file as one whose
 * names partly come from elsewhere (see `CROSS_UNIT_CASCADE`).
 */
const CROSS_UNIT_ROOTS = new Set([
  "bynk.uses.unknown_commons",
  "bynk.consumes.unknown_context",
  "bynk.suite.unknown_target",
]);

/**
 * Categories that single-file analysis gets wrong in any file. Each one depends
 * on something that isn't in the buffer:
 * - `bynk.tier.system_needs_wire`: the suite's target context.
 * - `bynk.stability.experimental`: the `bynk.toml` `[unstable] allow` list.
 * - `bynk.project.inconsistent_commons_name`: the file's real path (the
 *   in-memory one is synthesised).
 */
const SINGLE_FILE_ARTEFACTS = new Set([
  "bynk.tier.system_needs_wire",
  "bynk.stability.experimental",
  "bynk.project.inconsistent_commons_name",
]);

/**
 * In a file with a cross-unit root, or in one file of a unit split across
 * several, the categories that cascade from the missing names: unresolved
 * names, a consumed context's services and capabilities, its events, and the
 * type inference that needed those types.
 */
const CROSS_UNIT_CASCADE = [
  "bynk.resolve.",
  "bynk.consumes.",
  "bynk.given.unknown_capability",
  "bynk.event.unknown_subscription",
  "bynk.generics.type_arg_mismatch",
  "bynk.types.cannot_infer_",
  "bynk.types.uninferable_element_type",
];

/**
 * Whether `text` is one file of a unit split across several (`commons shapes`
 * in `shapes/operations.bynk`), judged from its `context`/`commons`/`adapter`
 * header against the file name in `uri`. A single-file unit `a.b` lives in
 * `a/b.bynk`. A buffer whose URI doesn't end in `.bynk` (an untitled one) is
 * taken as single-file.
 */
export function isPartOfMultiFileUnit(text: string, uri: string): boolean {
  const file = /([^/]+)\.bynk$/.exec(uri)?.[1];
  if (file === undefined) return false;
  const header = /^\s*(?:context|commons|adapter)\s+([A-Za-z_][\w.]*)/m.exec(text);
  if (!header) return false;
  const last = header[1].split(".").pop();
  return last !== decodeSegment(file);
}

/** A percent-decoded URI segment, or the segment as-is when it isn't valid. */
function decodeSegment(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}

/**
 * The diagnostics to publish for one single-file analysis, or `undefined` when
 * the analysis didn't run (a failed load, a trap, a poisoned instance).
 * Publishing an empty set then would wipe the last good diagnostics, as if the
 * file had become clean, so the caller leaves them in place.
 */
export function publishableDiagnostics(
  outcome: WasmOutcome<{ diagnostics: WasmDiagnostic[] }>,
  text: string,
  uri: string,
): LspDiagnostic[] | undefined {
  if (!outcome.ok) return undefined;
  return toLspDiagnostics(
    text,
    singleFileDiagnostics(outcome.value.diagnostics, isPartOfMultiFileUnit(text, uri)),
  );
}

/**
 * Drop the diagnostics that single-file analysis can't get right (DECISION B):
 *
 * - the cross-unit roots and the single-file artefacts, always;
 * - an adapter's unreadable binding module, always, because the worker reads
 *   no files. An adapter missing its `binding` clause is still reported;
 * - the cross-unit cascade, when the file has a root or is one file of a
 *   multi-file unit (`multiFileUnit`). Elsewhere a `resolve` or type error is
 *   real and is kept.
 *
 * The status bar labels the mode "Bynk (web, single-file)" so the gap is
 * visible. Project loading (slice 2) analyses a project's files together.
 */
export function singleFileDiagnostics(
  diags: WasmDiagnostic[],
  multiFileUnit = false,
): WasmDiagnostic[] {
  const cascade = multiFileUnit || diags.some((d) => CROSS_UNIT_ROOTS.has(d.category));
  return diags.filter((d) => {
    if (CROSS_UNIT_ROOTS.has(d.category) || SINGLE_FILE_ARTEFACTS.has(d.category)) return false;
    if (d.category === "bynk.adapter.no_binding" && d.message.includes("could not be read")) {
      return false;
    }
    return !(cascade && CROSS_UNIT_CASCADE.some((c) => d.category.startsWith(c)));
  });
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
