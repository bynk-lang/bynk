import { test } from "node:test";
import assert from "node:assert/strict";
import { wireRefusal } from "../src/boundary.ts";

// #4278: a `400 RefinementViolation` body names the field and the message and never the value
// that was rejected: a password failing `MinLength` must not be echoed back.

test("the violation keeps its field and message and loses its value", () => {
  const out = wireRefusal({
    kind: "RefinementViolation",
    path: "body.password",
    violation: { field: "Password", message: "length must be at least 12", value: "hunter2-the-password" },
  });
  assert.deepEqual(out, {
    kind: "RefinementViolation",
    path: "body.password",
    violation: { field: "Password", message: "length must be at least 12" },
  });
  assert.ok(!JSON.stringify(out).includes("hunter2"), "the offending value must not be echoed");
});

test("any other body passes through untouched", () => {
  const other = { kind: "StructuralMismatch", path: "body", expected: "Int", actual: "String" };
  assert.equal(wireRefusal(other), other);
  const malformed = { kind: "MalformedJson", details: "Invalid request body" };
  assert.equal(wireRefusal(malformed), malformed);
});
