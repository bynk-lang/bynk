// #2470: the explorer's candidate check, over every predicate form in
// `book/reference/refined-types.md`. The `check` strings are the ones
// `bynk-lsp`'s `refined_request.rs` tests pin from the emitter's lowering.

import * as assert from "assert";

import { checkCandidate, evaluateClauses, parseCandidate } from "../../src/refinedEval";
import type { RefinedClause, RefinedModel } from "../../src/webview/refinedTypes";

function clause(source: string, check: string, message: string): RefinedClause {
  return { source, description: "", check, message };
}

function model(base: RefinedModel["base"], clauses: RefinedClause[]): RefinedModel {
  return { name: "T", base, opaque: false, declaration: "", clauses, examples: [], examplesNote: null, seed: 0 };
}

const NON_NEGATIVE = clause("NonNegative", "value >= 0", "must be non-negative");
const POSITIVE = clause("Positive", "value > 0", "must be positive");
const IN_RANGE = clause("InRange(1, 100)", "value >= 1 && value <= 100", "must be in range [1, 100]");
const IN_RANGE_F = clause("InRange(0.5, 2.5)", "value >= 0.5 && value <= 2.5", "must be in range [0.5, 2.5]");
const NON_EMPTY = clause("NonEmpty", "value.length > 0", "must be non-empty");
const MIN_LENGTH = clause("MinLength(3)", "value.length >= 3", "length must be at least 3");
const MAX_LENGTH = clause("MaxLength(5)", "value.length <= 5", "length must be at most 5");
const LENGTH = clause("Length(4)", "value.length === 4", "length must be exactly 4");
const MATCHES = clause(
  "Matches(\"[a-z]+\")",
  'new globalThis.RegExp("^(?:" + "[a-z]+" + ")$").test(value)',
  "must match /[a-z]+/",
);

function failed(m: RefinedModel, text: string): string[] {
  const v = checkCandidate(m, text);
  assert.strictEqual(v.kind, "invalid", `${text} should fail`);
  return v.kind === "invalid" ? v.failures.map((f) => f.source) : [];
}

describe("refinedEval (unit)", () => {
  it("NonNegative", () => {
    const m = model("Int", [NON_NEGATIVE]);
    assert.strictEqual(checkCandidate(m, "0").kind, "valid");
    assert.deepStrictEqual(failed(m, "-1"), ["NonNegative"]);
  });

  it("Positive", () => {
    const m = model("Int", [POSITIVE]);
    assert.strictEqual(checkCandidate(m, "1").kind, "valid");
    assert.deepStrictEqual(failed(m, "0"), ["Positive"]);
  });

  it("InRange over Int, inclusive at both ends", () => {
    const m = model("Int", [IN_RANGE]);
    for (const ok of ["1", "100", " 50 ", "+7"]) assert.strictEqual(checkCandidate(m, ok).kind, "valid", ok);
    for (const bad of ["0", "101", "-3"]) assert.deepStrictEqual(failed(m, bad), ["InRange(1, 100)"]);
  });

  it("InRange over Float", () => {
    const m = model("Float", [IN_RANGE_F]);
    for (const ok of ["0.5", "2.5", "1", "1e0"]) assert.strictEqual(checkCandidate(m, ok).kind, "valid", ok);
    for (const bad of ["0.49", "2.51"]) assert.deepStrictEqual(failed(m, bad), ["InRange(0.5, 2.5)"]);
  });

  it("NonEmpty, MinLength, MaxLength and Length", () => {
    assert.strictEqual(checkCandidate(model("String", [NON_EMPTY]), "a").kind, "valid");
    assert.deepStrictEqual(failed(model("String", [NON_EMPTY]), ""), ["NonEmpty"]);
    assert.strictEqual(checkCandidate(model("String", [MIN_LENGTH]), "abc").kind, "valid");
    assert.deepStrictEqual(failed(model("String", [MIN_LENGTH]), "ab"), ["MinLength(3)"]);
    assert.strictEqual(checkCandidate(model("String", [MAX_LENGTH]), "abcde").kind, "valid");
    assert.deepStrictEqual(failed(model("String", [MAX_LENGTH]), "abcdef"), ["MaxLength(5)"]);
    assert.strictEqual(checkCandidate(model("String", [LENGTH]), "abcd").kind, "valid");
    assert.deepStrictEqual(failed(model("String", [LENGTH]), "abc"), ["Length(4)"]);
  });

  it("Matches is anchored to the whole string", () => {
    const m = model("String", [MATCHES]);
    assert.strictEqual(checkCandidate(m, "abc").kind, "valid");
    assert.deepStrictEqual(failed(m, "abc1"), ["Matches(\"[a-z]+\")"]);
    assert.deepStrictEqual(failed(m, "1abc"), ["Matches(\"[a-z]+\")"]);
    assert.deepStrictEqual(failed(m, ""), ["Matches(\"[a-z]+\")"]);
  });

  it("a string candidate is taken verbatim, spaces included", () => {
    assert.strictEqual(checkCandidate(model("String", [MIN_LENGTH]), "   ").kind, "valid");
  });

  it("every failing clause is named, in check order", () => {
    const m = model("String", [MIN_LENGTH, MATCHES]);
    assert.deepStrictEqual(failed(m, "A"), ["MinLength(3)", "Matches(\"[a-z]+\")"]);
    assert.deepStrictEqual(failed(m, "ABC"), ["Matches(\"[a-z]+\")"]);
    const v = checkCandidate(m, "A");
    assert.ok(v.kind === "invalid" && v.failures[0].message === "length must be at least 3");
  });

  it("unreadable candidates run no clause", () => {
    assert.strictEqual(checkCandidate(model("Int", [POSITIVE]), "abc").kind, "unreadable");
    assert.strictEqual(checkCandidate(model("Int", [POSITIVE]), "1.5").kind, "unreadable");
    assert.strictEqual(checkCandidate(model("Int", [POSITIVE]), "").kind, "unreadable");
    assert.strictEqual(checkCandidate(model("Int", [POSITIVE]), "9223372036854775808").kind, "unreadable");
    assert.strictEqual(checkCandidate(model("Float", [NON_NEGATIVE]), "NaN").kind, "unreadable");
    assert.strictEqual(checkCandidate(model("Float", [NON_NEGATIVE]), "1e999").kind, "unreadable");
  });

  it("an Int is a bigint: the emitted comparisons against number literals still hold", () => {
    const m = model("Int", [IN_RANGE]);
    assert.strictEqual(checkCandidate(m, "100").kind, "valid");
    assert.strictEqual(checkCandidate(m, "9000000000000000000").kind, "invalid");
  });

  it("a runaway clause is stopped, not waited on", () => {
    const slow = clause("Slow", "(() => { for (;;) {} })()", "");
    const started = Date.now();
    assert.strictEqual(evaluateClauses([slow], 1n, 50).kind, "timeout");
    assert.ok(Date.now() - started < 2000);
  });

  it("the context has no string eval, no require, no process", () => {
    const probe = (code: string) => evaluateClauses([clause("p", code, "")], 1n).kind;
    assert.strictEqual(probe("typeof require === 'undefined' && typeof process === 'undefined'"), "valid");
    assert.throws(() => evaluateClauses([clause("p", "eval('1')", "")], 1n));
  });

  it("a hostile candidate is only ever a value, never code", () => {
    const hostile = ["1); process.exit(1); (", "0 || (globalThis.pwned = 1)", "`${process.exit(1)}`", "\"; process.exit(1); \""];
    for (const text of hostile) {
      // As a String candidate: a failing or passing value, with no side effect.
      const s = checkCandidate(model("String", [MIN_LENGTH, MATCHES]), text);
      assert.ok(s.kind === "valid" || s.kind === "invalid", text);
      // As an Int or Float: not a number, so no clause runs at all.
      assert.strictEqual(checkCandidate(model("Int", [IN_RANGE]), text).kind, "unreadable", text);
      assert.strictEqual(checkCandidate(model("Float", [IN_RANGE_F]), text).kind, "unreadable", text);
    }
    assert.strictEqual((globalThis as { pwned?: number }).pwned, undefined);
  });

  it("a clause that throws still yields a verdict for the text", () => {
    const broken = clause("Matches(\"(\")", 'new globalThis.RegExp("^(?:" + "(" + ")$").test(value)', "");
    const v = checkCandidate(model("String", [broken]), "abc");
    assert.strictEqual(v.kind, "unreadable");
    assert.ok(v.kind === "unreadable" && v.message.includes("could not be checked"), JSON.stringify(v));
  });

  it("parseCandidate reads each base", () => {
    assert.deepStrictEqual(parseCandidate("Int", "-12"), { ok: true, value: -12n });
    assert.deepStrictEqual(parseCandidate("Float", ".5"), { ok: true, value: 0.5 });
    assert.deepStrictEqual(parseCandidate("String", " x "), { ok: true, value: " x " });
  });
});
