import { test } from "node:test";
import assert from "node:assert/strict";
import { wireRefusal } from "../src/boundary.ts";
import type { ValidationRule } from "../src/errors.ts";

// #3952 (Decision D): the `400` body names the field, the message and the rule, and never the
// value that was rejected: a password failing `MinLength` must not be echoed back.

const body = (rule: ValidationRule, value: unknown) => ({
  kind: "RefinementViolation",
  path: "body.note",
  violation: { field: "Note", message: "length must be at most 280", rule, value },
});

test("the rule is on the wire and the value is not", () => {
  const out = wireRefusal(body({ tag: "MaxLength", limit: 280 }, "hunter2-the-password"));
  assert.deepEqual(out, {
    kind: "RefinementViolation",
    path: "body.note",
    violation: {
      field: "Note",
      message: "length must be at most 280",
      rule: { kind: "MaxLength", limit: 280 },
    },
  });
  assert.ok(!JSON.stringify(out).includes("hunter2"), "the offending value must not be echoed");
  assert.ok(!("value" in (out.violation as object)));
});

test("a sum is written with `kind` on the wire, bounds included", () => {
  const out = wireRefusal(
    body(
      {
        tag: "InRange",
        lo: { tag: "IntBound", value: 1 },
        hi: { tag: "IntBound", value: 100 },
      },
      0,
    ),
  );
  assert.deepEqual((out.violation as { rule: unknown }).rule, {
    kind: "InRange",
    lo: { kind: "IntBound", value: 1 },
    hi: { kind: "IntBound", value: 100 },
  });
});

test("a Decimal bound is its canonical text and a Duration its milliseconds", () => {
  const decimal = wireRefusal(
    body({ tag: "AtMost", bound: { tag: "DecimalBound", value: { coefficient: 9950n, scale: 2 } } }, 1),
  );
  assert.deepEqual((decimal.violation as { rule: unknown }).rule, {
    kind: "AtMost",
    bound: { kind: "DecimalBound", value: "99.50" },
  });
  const duration = wireRefusal(
    body({ tag: "AtLeast", bound: { tag: "DurationBound", value: 5000 } }, 1),
  );
  assert.deepEqual((duration.violation as { rule: unknown }).rule, {
    kind: "AtLeast",
    bound: { kind: "DurationBound", value: 5000 },
  });
});

test("a rule with no argument is just its kind", () => {
  const out = wireRefusal(body({ tag: "NonEmpty" }, ""));
  assert.deepEqual((out.violation as { rule: unknown }).rule, { kind: "NonEmpty" });
});

test("any other body passes through untouched", () => {
  const other = { kind: "StructuralMismatch", path: "body", expected: "Int", actual: "String" };
  assert.equal(wireRefusal(other), other);
});
