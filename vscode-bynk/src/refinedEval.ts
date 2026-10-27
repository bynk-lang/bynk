// #2470: checks a candidate against a refined type's clauses, for the explorer
// panel (`refinedExplorer.ts`). Pure of `vscode`, so it unit-tests directly.
//
// Each clause's `check` is the condition the emitter lowers for `.of`, printed
// by the server as a JavaScript expression over `value`. Running it here is
// DECISION A of #2470: no project code is evaluated — the text comes from a
// closed set of eight predicate forms the compiler generates, the one
// user-written part (a `Matches` pattern) sits inside a string literal, and the
// candidate is bound as `value`, never spliced into source. It runs in a fresh
// `node:vm` context with string `eval` disabled and a time limit, so a slow
// pattern cannot hang the extension host. (`vm` is not a security boundary; the
// guarantee is that nothing but compiler-generated predicates reaches it.)

import * as vm from "node:vm";

import type { CandidateVerdict, RefinedClause, RefinedModel } from "./webview/refinedTypes";

/** How long one clause may run. */
export const CLAUSE_TIMEOUT_MS = 250;

const INT_MIN = -(2n ** 63n);
const INT_MAX = 2n ** 63n - 1n;

/** Read `text` as a value of `base`, as a hand-typed candidate. A `String` is
 *  taken verbatim — spaces count — and an `Int` is a `bigint`, as the emitted
 *  code holds one. */
export function parseCandidate(
  base: RefinedModel["base"],
  text: string,
): { ok: true; value: bigint | number | string } | { ok: false; message: string } {
  const t = text.trim();
  switch (base) {
    case "String":
      return { ok: true, value: text };
    case "Int": {
      if (!/^[+-]?\d+$/.test(t)) return { ok: false, message: "Not an Int: write a whole number." };
      const n = BigInt(t);
      if (n < INT_MIN || n > INT_MAX) return { ok: false, message: "Outside the 64-bit Int range." };
      return { ok: true, value: n };
    }
    case "Float": {
      if (!/^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(t)) {
        return { ok: false, message: "Not a Float: write a decimal number." };
      }
      const n = Number(t);
      return Number.isFinite(n)
        ? { ok: true, value: n }
        : { ok: false, message: "Not a finite Float." };
    }
  }
}

/** Evaluate every clause against `value`; all failures are reported. */
export function evaluateClauses(
  clauses: readonly RefinedClause[],
  value: bigint | number | string,
  timeoutMs: number = CLAUSE_TIMEOUT_MS,
): CandidateVerdict {
  const failures: Array<{ source: string; message: string }> = [];
  for (const clause of clauses) {
    const context = vm.createContext(
      { value },
      { codeGeneration: { strings: false, wasm: false } },
    );
    let holds: unknown;
    try {
      holds = vm.runInContext(clause.check, context, { timeout: timeoutMs });
    } catch (err) {
      if ((err as { code?: string }).code === "ERR_SCRIPT_EXECUTION_TIMEOUT") {
        return { kind: "timeout" };
      }
      throw err;
    }
    if (holds !== true) failures.push({ source: clause.source, message: clause.message });
  }
  return failures.length === 0 ? { kind: "valid" } : { kind: "invalid", failures };
}

/** The panel's check: read the text, then run the clauses. */
export function checkCandidate(model: RefinedModel, text: string): CandidateVerdict {
  const parsed = parseCandidate(model.base, text);
  if (!parsed.ok) return { kind: "unreadable", message: parsed.message };
  try {
    return evaluateClauses(model.clauses, parsed.value);
  } catch (err) {
    // A clause that throws (a `Matches` pattern V8's `RegExp` rejects though the
    // compiler accepted it) still owes the panel an answer for this text.
    return { kind: "unreadable", message: `This value could not be checked: ${String(err)}` };
  }
}
