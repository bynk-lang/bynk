// #855: unit coverage for `wire-render.ts`'s pure sentence/label generators —
// the DOM-free part of the wire-contract peek, testable directly against
// hand-built `Wc*` values without a live webview render (this repo's test
// harness runs Mocha inside the extension host, a plain Node process with no
// `document` — see `wire-render.ts`'s module doc for why the DOM-construction
// half of that file has no direct test, the same posture `mermaid-gen.unit
// .test.ts` (#846) and `architecture-gen.unit.test.ts` (#851) establish for
// their own features).

import * as assert from "assert";

import {
  bareEnvelopeSentence,
  contractMismatchSentence,
  emptyEnvelopeSentence,
  formatHeaderLine,
  jsonKindWire,
  noCrossContextSentence,
  rawEnvelopeSentence,
  responseRowMuted,
  responseRowWhy,
  revalidationSentence,
} from "../../src/webview/wire-render";

describe("formatHeaderLine", () => {
  it("renders an HTTP handler's method + route literal", () => {
    assert.strictEqual(
      formatHeaderLine({ kind: "Http", method: "GET", path: "/check/:client" }),
      'on GET("/check/:client")',
    );
  });

  it("renders a call handler with no params/route to show", () => {
    assert.strictEqual(formatHeaderLine({ kind: "Call" }), "on call");
  });

  it("renders a cron handler's expression", () => {
    assert.strictEqual(
      formatHeaderLine({ kind: "Cron", expr: "0 * * * *" }),
      'on cron("0 * * * *")',
    );
  });
});

describe("envelope sentences", () => {
  it("a Bare envelope renders the correct sentence", () => {
    assert.strictEqual(
      bareEnvelopeSentence("client"),
      "The request body is the value of `client`.",
    );
  });

  it("a Raw envelope says the body is octets under the declared type", () => {
    assert.strictEqual(
      rawEnvelopeSentence("body", "application/vnd.bynk.tree-v1"),
      "The request body is the raw octets of `body` — not JSON, not base64 — sent as `application/vnd.bynk.tree-v1`.",
    );
    assert.ok(rawEnvelopeSentence("body", "*/*").endsWith("sent as any content type."));
  });

  it("an Empty envelope explains the body is empty, not just absent", () => {
    assert.ok(emptyEnvelopeSentence().toLowerCase().includes("empty"));
  });
});

describe("revalidationSentence", () => {
  it("a StructuralOnly scalar renders the not-re-validated line, not a re-validated one", () => {
    const sentence = revalidationSentence("OpaqueToken", "String", "StructuralOnly", ["NonEmpty"]);
    assert.ok(sentence.includes("not re-validated here"), sentence);
    assert.ok(sentence.includes("opaque to this context"), sentence);
    assert.ok(sentence.includes("contract hash"), sentence);
    // Must not accidentally claim re-validation for an opaque/consumed type.
    assert.ok(!sentence.includes("never trusted from the caller"), sentence);
  });

  it("a ViaConstructor scalar names its predicates and the wire kind", () => {
    const sentence = revalidationSentence("ClientId", "String", "ViaConstructor", ["NonEmpty"]);
    assert.strictEqual(
      sentence,
      "ClientId → wire string, re-validated NonEmpty at the boundary (never trusted from the caller).",
    );
  });

  it("an Inline scalar with multiple predicates lists them all, declaration order", () => {
    const sentence = revalidationSentence("Score", "Number", "Inline", ["NonNegative", "MaxLength(3)"]);
    assert.ok(sentence.includes("NonNegative, MaxLength(3)"), sentence);
  });

  it("a Base64Decode scalar states decoding, not casting", () => {
    const sentence = revalidationSentence("Blob", "String", "Base64Decode", []);
    assert.ok(sentence.includes("base64"), sentence);
    assert.ok(sentence.includes("decoded not cast"), sentence);
  });
});

describe("contract sentences", () => {
  it("states the 409 ContractMismatch consequence", () => {
    assert.ok(contractMismatchSentence().includes("409 ContractMismatch"));
  });

  it("explains a single-context project has nothing to cross", () => {
    assert.ok(noCrossContextSentence("SingleContext").toLowerCase().includes("single context"));
  });

  it("explains a non-call handler has no contract form", () => {
    assert.ok(noCrossContextSentence("NotACallHandler").includes("on call"));
  });
});

describe("jsonKindWire", () => {
  it("lowercases the wire JSON kind", () => {
    assert.strictEqual(jsonKindWire("String"), "string");
    assert.strictEqual(jsonKindWire("Number"), "number");
    assert.strictEqual(jsonKindWire("Boolean"), "boolean");
  });
});

describe("response row helpers", () => {
  it("mutes only BoundaryImplicit rows", () => {
    assert.strictEqual(responseRowMuted({ kind: "DeclaredSuccess" }), false);
    assert.strictEqual(responseRowMuted({ kind: "Constructed", range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } } }), false);
    assert.strictEqual(responseRowMuted({ kind: "BoundaryImplicit", why: "any param present" }), true);
  });

  it("carries the boundary-implicit reason and nothing for other origins", () => {
    assert.strictEqual(responseRowWhy({ kind: "BoundaryImplicit", why: "any param present" }), "any param present");
    assert.strictEqual(responseRowWhy({ kind: "DeclaredSuccess" }), null);
  });
});
