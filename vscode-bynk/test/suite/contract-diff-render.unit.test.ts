// #2467: unit coverage for `contract-diff-render.ts`'s DOM-free sentence
// helpers (the DOM half has no direct test, like `wire-render.ts`'s).

import * as assert from "assert";

import {
  changeLabel,
  ledgerSentence,
  serviceSentence,
  summarySentence,
} from "../../src/webview/contract-diff-render";
import type { CdChange, CdModel, CdService } from "../../src/webview/contractDiffTypes";

const change = (name: string, cls: CdChange["class"]): CdChange => ({
  kind: "event",
  unit: "commerce.order",
  name,
  class: cls,
  lockedVersion: 1,
  detail: "field(s) removed: email",
  loc: null,
});

const model = (over: Partial<CdModel>): CdModel => ({
  lockPresent: true,
  ledger: "read",
  changes: [],
  services: [],
  ...over,
});

describe("summarySentence", () => {
  it("says so when nothing changed", () => {
    assert.strictEqual(summarySentence(model({})), "No boundary changes since the lock.");
  });

  it("counts breaking and compatible changes apart", () => {
    const m = model({
      changes: [change("A", "breaking"), change("B", "compatible"), change("C", "compatible")],
    });
    assert.strictEqual(summarySentence(m), "1 breaking, 2 compatible.");
  });

  it("counts service contracts as changed, never breaking", () => {
    const svc: CdService = {
      environment: "default",
      context: "shop.billing",
      service: "charge",
      status: "changed",
      live: "aaaaaaaaaaaaaaaa",
      current: "bbbbbbbbbbbbbbbb",
      loc: null,
    };
    assert.strictEqual(summarySentence(model({ services: [svc, svc] })), "2 service contracts changed.");
    assert.ok(!summarySentence(model({ services: [svc] })).includes("breaking"));
  });
});

describe("labels", () => {
  it("names a change by kind and qualified name", () => {
    assert.strictEqual(changeLabel(change("PaymentConfirmed", "breaking")), "event commerce.order.PaymentConfirmed");
  });

  it("shortens hashes in a service sentence", () => {
    const s: CdService = {
      environment: "default",
      context: "shop.billing",
      service: "charge",
      status: "changed",
      live: "aaaaaaaaaaaaaaaa",
      current: "bbbbbbbbbbbbbbbb",
      loc: null,
    };
    assert.strictEqual(serviceSentence(s), "charge: contract aaaaaaaa → bbbbbbbb");
    assert.ok(serviceSentence({ ...s, status: "added", live: null }).includes("not in the live record"));
    assert.ok(serviceSentence({ ...s, status: "removed", current: null }).includes("no longer provided"));
  });
});

describe("ledgerSentence", () => {
  it("explains each way the ledger can be missing", () => {
    assert.ok(ledgerSentence("absent", 0)?.includes("no bynk.deploy.lock"));
    assert.ok(ledgerSentence("unreadable", 0)?.includes("could not be read"));
  });

  it("is silent when there are rows to show", () => {
    assert.strictEqual(ledgerSentence("read", 2), null);
    assert.ok(ledgerSentence("read", 0)?.includes("No service contract differs"));
  });
});
