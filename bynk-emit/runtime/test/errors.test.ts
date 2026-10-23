import { test } from "node:test";
import assert from "node:assert/strict";
import { invariantViolation, type InvariantViolation } from "../src/errors.ts";

const recordOf = (e: Error): InvariantViolation | undefined =>
  (e as { invariantViolation?: InvariantViolation }).invariantViolation;

test("invariantViolation: the message and record name the agent and the declaration", () => {
  const e = invariantViolation("Wallet", "nonneg");
  assert.equal(e.message, "InvariantViolation: Wallet.nonneg");
  assert.deepEqual(recordOf(e), { kind: "InvariantViolation", agent: "Wallet", invariant: "nonneg" });
});

test("invariantViolation: says whether an invariant or a transition was violated", () => {
  assert.equal(recordOf(invariantViolation("Wallet", "nonneg", "invariant"))?.declaration, "invariant");
  assert.equal(recordOf(invariantViolation("Ledger", "never_decreases", "transition"))?.declaration, "transition");
});

test("invariantViolation: one name for both kinds gives the same message and different records", () => {
  // The checker lets an agent declare an invariant and a transition under one name,
  // so only the record can tell which fired.
  const inv = invariantViolation("Meter", "same", "invariant");
  const tr = invariantViolation("Meter", "same", "transition");
  assert.equal(inv.message, tr.message);
  assert.notDeepEqual(recordOf(inv), recordOf(tr));
});

test("invariantViolation: a caller that passes no declaration leaves it out of the record", () => {
  const record = recordOf(invariantViolation("Wallet", "nonneg"));
  assert.ok(record !== undefined);
  assert.equal("declaration" in record, false);
});
