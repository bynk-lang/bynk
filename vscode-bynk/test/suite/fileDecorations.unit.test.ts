// #2478: the Explorer decorations' badge rules and failing-test bookkeeping
// (`fileDecorationsCore.ts`), `vscode`-free.

import * as assert from "assert";

import {
  FailureTracker,
  abbreviate,
  contextBadge,
  type ScannedFile,
} from "../../src/fileDecorationsCore";

const file = (kind: ScannedFile["kind"], name: string, root = "file:///p"): ScannedFile => ({ root, kind, name });

describe("file decorations (#2478)", () => {
  describe("abbreviate", () => {
    it("takes the first two letters of a single word", () => {
      assert.strictEqual(abbreviate("billing"), "BI");
      assert.strictEqual(abbreviate("a"), "A");
    });
    it("takes the initials of a multi-word name", () => {
      assert.strictEqual(abbreviate("order.billing"), "OB");
      assert.strictEqual(abbreviate("order_billing"), "OB");
      assert.strictEqual(abbreviate("OrderBilling"), "OB");
      assert.strictEqual(abbreviate("a.b.c"), "AB");
    });
  });

  describe("contextBadge", () => {
    const two = new Map<string, ScannedFile>([
      ["file:///p/billing.bynk", file("context", "billing")],
      ["file:///p/store.bynk", file("context", "storefront")],
      ["file:///p/billing_test.bynk", file("test", "billing")],
      ["file:///p/orphan_test.bynk", file("test", "missing")],
      ["file:///p/money.bynk", file("commons", "money")],
      ["file:///q/solo.bynk", file("context", "solo", "file:///q")],
    ]);

    it("badges context files, with the full name for the tooltip", () => {
      assert.deepStrictEqual(contextBadge(two, "file:///p/billing.bynk", "auto"), { badge: "BI", context: "billing" });
    });
    it("badges a suite with the context it tests", () => {
      assert.strictEqual(contextBadge(two, "file:///p/billing_test.bynk", "auto")?.context, "billing");
    });
    it("leaves commons, unmatched suites and unknown files bare", () => {
      assert.strictEqual(contextBadge(two, "file:///p/money.bynk", "auto"), undefined);
      assert.strictEqual(contextBadge(two, "file:///p/orphan_test.bynk", "auto"), undefined);
      assert.strictEqual(contextBadge(two, "file:///p/none.bynk", "auto"), undefined);
    });
    it("is off for a single-context project unless always", () => {
      assert.strictEqual(contextBadge(two, "file:///q/solo.bynk", "auto"), undefined);
      assert.strictEqual(contextBadge(two, "file:///q/solo.bynk", "always")?.badge, "SO");
    });
    it("is off when never", () => {
      assert.strictEqual(contextBadge(two, "file:///p/billing.bynk", "never"), undefined);
    });
  });

  describe("FailureTracker", () => {
    it("tints a file on a failure and clears it when the case passes", () => {
      const t = new FailureTracker();
      const seen: string[][] = [];
      t.onDidChange((u) => seen.push(u));
      t.record("f", "a", "failed");
      assert.ok(t.hasFailures("f"));
      t.record("f", "a", "passed");
      assert.ok(!t.hasFailures("f"));
      assert.deepStrictEqual(seen, [["f"], ["f"]]);
    });
    it("keeps the tint while another case in the file still fails", () => {
      const t = new FailureTracker();
      t.record("f", "a", "failed");
      t.record("f", "b", "failed");
      t.record("f", "a", "passed");
      assert.ok(t.hasFailures("f"));
      t.record("f", "b", "passed");
      assert.ok(!t.hasFailures("f"));
    });
    it("does not fire for a repeat or for a pass that was never a failure", () => {
      const t = new FailureTracker();
      let n = 0;
      t.onDidChange(() => n++);
      t.record("f", "a", "passed");
      t.record("f", "a", "failed");
      t.record("f", "a", "failed");
      assert.strictEqual(n, 1);
    });
    it("ignores a case with no file", () => {
      const t = new FailureTracker();
      t.record(undefined, "a", "failed");
      assert.ok(!t.hasFailures("undefined"));
    });
    it("forgets a failed case that no longer exists", () => {
      const t = new FailureTracker();
      t.record("f", "a", "failed");
      t.record("g", "b", "failed");
      t.retain(new Set(["b"]));
      assert.ok(!t.hasFailures("f"));
      assert.ok(t.hasFailures("g"));
    });
  });
});
