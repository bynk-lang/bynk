// Unit coverage for the web worker's bynk-wasm → LSP conversions (#2102):
// diagnostics (with the single-file filter, DECISION B), hover and completion.

import * as assert from "assert";

import {
  hoverMarkdown,
  singleFileDiagnostics,
  toLspCompletionItems,
  toLspDiagnostics,
  type WasmDiagnostic,
} from "../../src/web/convert";

const diag = (category: string, from = 0, to = 1): WasmDiagnostic => ({
  from,
  to,
  severity: "error",
  category,
  message: category,
  notes: [],
  labels: [],
});

describe("web singleFileDiagnostics (DECISION B)", () => {
  it("keeps a resolve error in a file with no cross-unit reference", () => {
    const kept = singleFileDiagnostics([diag("bynk.resolve.unknown_name")]);
    assert.deepStrictEqual(
      kept.map((d) => d.category),
      ["bynk.resolve.unknown_name"],
    );
  });

  it("drops an unresolved `uses` and the resolve errors it cascades into", () => {
    const kept = singleFileDiagnostics([
      diag("bynk.uses.unknown_commons"),
      diag("bynk.resolve.unknown_type"),
      diag("bynk.resolve.unknown_function"),
      diag("bynk.check.type_mismatch"),
    ]);
    assert.deepStrictEqual(
      kept.map((d) => d.category),
      ["bynk.check.type_mismatch"],
    );
  });

  it("drops a suite's unresolved target", () => {
    const kept = singleFileDiagnostics([diag("bynk.suite.unknown_target")]);
    assert.deepStrictEqual(kept, []);
  });
});

describe("web toLspDiagnostics", () => {
  it("ranges come from the byte span, in UTF-16 positions", () => {
    // `é` is two bytes: the span of `x` is bytes 3..4, UTF-16 character 2..3.
    const [d] = toLspDiagnostics("é x", [
      { ...diag("bynk.parse.expected_token", 3, 4), severity: "warning", message: "m", notes: ["n"] },
    ]);
    assert.deepStrictEqual(d.range, {
      start: { line: 0, character: 2 },
      end: { line: 0, character: 3 },
    });
    assert.strictEqual(d.severity, 2);
    assert.strictEqual(d.code, "bynk.parse.expected_token");
    assert.strictEqual(d.source, "bynk");
    assert.strictEqual(d.message, "m\nn");
  });

  it("maps errors to Error severity and repairs an inverted span", () => {
    const [d] = toLspDiagnostics("abc", [diag("c", 2, 1)]);
    assert.strictEqual(d.severity, 1);
    assert.deepStrictEqual(d.range.start, d.range.end);
  });
});

describe("web hoverMarkdown", () => {
  it("fences the type as bynk, like bynk-lsp", () => {
    assert.strictEqual(hoverMarkdown("Instant"), "```bynk\nInstant\n```");
    assert.strictEqual(hoverMarkdown(null), undefined);
    assert.strictEqual(hoverMarkdown(""), undefined);
  });
});

describe("web toLspCompletionItems", () => {
  it("uses bynk-lsp's kind table and marks snippet items", () => {
    const [cap, snippet, local, unknown] = toLspCompletionItems([
      { label: "Clock", kind: "capability", detail: "capability", insert_text: null },
      { label: "match", kind: "snippet", detail: null, insert_text: "match ${1:x} {}" },
      { label: "now", kind: "local", detail: "Instant", insert_text: null },
      { label: "?", kind: "mystery", detail: null, insert_text: null },
    ]);
    assert.deepStrictEqual(cap, { label: "Clock", kind: 8, detail: "capability" });
    assert.deepStrictEqual(snippet, {
      label: "match",
      kind: 15,
      insertText: "match ${1:x} {}",
      insertTextFormat: 2,
    });
    assert.strictEqual(local.kind, 6);
    assert.strictEqual(unknown.kind, 1);
  });
});
