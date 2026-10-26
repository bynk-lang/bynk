// #2463: unit coverage for `lmRender` (src/lmRender.ts), the pure core of the
// language-model tools — each model as the compact Markdown an agent reads,
// with 1-based `path:line:col` locations, and the output cap.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import {
  capText,
  fmtLoc,
  renderArchitecture,
  renderDiagnostics,
  renderDocumentation,
  renderExplanation,
  renderSymbol,
  renderWireContract,
} from "../../src/lmRender";
import type { ArchModel } from "../../src/webview/architectureTypes";
import type { DocModel } from "../../src/webview/docTypes";
import type { WcModel } from "../../src/webview/wireContractTypes";

/** Strip the fixtures' `file:///p/` root, as `asRelativePath` would. */
const rel = (uri: string) => uri.replace(/^file:\/\/\/p\//, "");

const range = (line: number, character: number) => ({
  start: { line, character },
  end: { line, character: character + 1 },
});

describe("lmRender", () => {
  it("formats a 0-based range as a 1-based path:line:col", () => {
    assert.strictEqual(fmtLoc("src/a.bynk", range(0, 0)), "src/a.bynk:1:1");
    assert.strictEqual(fmtLoc("src/a.bynk", range(4, 8)), "src/a.bynk:5:9");
  });

  describe("capText", () => {
    it("leaves short text alone", () => {
      assert.strictEqual(capText("one\ntwo\n", 100), "one\ntwo\n");
    });

    it("cuts long text at a line boundary and says how to narrow", () => {
      const text = Array.from({ length: 50 }, (_, i) => `line ${i}`).join("\n");
      const out = capText(text, 40, "Pass `file`.");
      assert.ok(out.startsWith("line 0\nline 1\n"), out);
      assert.ok(!out.includes("line 49"), out);
      assert.match(out, /…\(truncated: \d+ more characters\. Pass `file`\.\)$/);
    });
  });

  it("renders an explanation with its Book link", () => {
    const out = renderExplanation({
      title: "bynk.resolve.unknown_type",
      markdown: "A type name was referenced that does not exist.\n",
      href: "https://bynk-lang.org/book/reference/types/",
    });
    assert.strictEqual(
      out,
      "# `bynk.resolve.unknown_type`\n\nA type name was referenced that does not exist.\n\n" +
        "Book: https://bynk-lang.org/book/reference/types/\n",
    );
  });

  describe("renderDiagnostics", () => {
    it("says when there is nothing", () => {
      assert.strictEqual(renderDiagnostics([], "in the workspace"), "No Bynk diagnostics in the workspace.\n");
    });

    it("lists errors before warnings, counts both, and points at the explain tool", () => {
      const out = renderDiagnostics(
        [
          { path: "src/b.bynk", line: 2, column: 1, severity: "warning", code: "bynk.given.unused_capability", message: "unused" },
          { path: "src/a.bynk", line: 3, column: 5, severity: "error", code: "bynk.resolve.unknown_type", message: "unknown type `Usr`" },
        ],
        "in the workspace",
      );
      const lines = out.split("\n");
      assert.strictEqual(lines[0], "1 error(s), 1 warning(s) in the workspace.");
      assert.strictEqual(lines[2], "- src/a.bynk:3:5 error[bynk.resolve.unknown_type]: unknown type `Usr`");
      assert.strictEqual(lines[3], "- src/b.bynk:2:1 warning[bynk.given.unused_capability]: unused");
      assert.ok(out.includes("`bynk_explain_diagnostic`"), out);
    });
  });

  it("renders documentation with each entry's kind, location and doc", () => {
    const model: DocModel = {
      unitName: "shop",
      unitKind: "context",
      unitDoc: "The shop.",
      unitRange: range(0, 0),
      entries: [
        { name: "Order", kind: "type", depth: 0, markdown: "```bynk\ntype Order = { id: String }\n```", documented: false, range: range(2, 5), uri: "file:///p/src/shop.bynk" },
        { name: "place", kind: "handler", depth: 1, markdown: "```bynk\non call place()\n```\n\nPlaces it.", documented: true, range: range(6, 10), uri: "file:///p/src/shop.bynk" },
      ],
    };
    const out = renderDocumentation(model, rel);
    assert.ok(out.startsWith("# context `shop`\n\nThe shop.\n"), out);
    assert.ok(out.includes("## type `Order` (src/shop.bynk:3:6)"), out);
    assert.ok(out.includes("_(no doc comment)_"), out);
    assert.ok(out.includes("### handler `place` (src/shop.bynk:7:11)"), out);
    assert.ok(out.includes("Places it."), out);
  });

  it("renders the architecture fixture: units, members and consumes edges", () => {
    const fixture = path.resolve(__dirname, "../../../test/fixtures/architecture-model.json");
    const model = JSON.parse(fs.readFileSync(fixture, "utf8")) as ArchModel;
    const out = renderArchitecture(model, rel);
    assert.match(out, new RegExp(`^# Architecture: ${model.nodes.length} unit\\(s\\), ${model.edges.length} dependency edge\\(s\\)`));
    assert.ok(out.includes("## context `storefront` (src/storefront/web.bynk:1:9)"), out);
    assert.ok(out.includes("- service `checkout`: 2 handler(s) (src/storefront/web.bynk:6:9)"), out);
    assert.ok(out.includes("- agent `Cart`: 1 handler(s) (src/storefront/cart.bynk:3:7)"), out);
    assert.ok(out.includes("- capability `Kv` (from `bynk.cloudflare`)"), out);
    for (const e of model.edges) {
      assert.ok(out.includes(`- consumes \`${e.to}\``), `edge ${e.from} → ${e.to} in:\n${out}`);
    }
  });

  it("renders a wire contract: route, request, responses and boundary types", () => {
    const model: WcModel = {
      unit: "shop",
      service: "api",
      kind: { kind: "Http", method: "POST", path: "/orders" },
      range: range(9, 2),
      handlerLine: 9,
      envelope: { kind: "Bare", param: "body", shape: { kind: "Named", name: "NewOrder" } },
      contract: null,
      types: [
        {
          name: "NewOrder",
          codecSuffix: "NewOrder",
          provenance: { kind: "Owned" },
          body: {
            kind: "Record",
            fields: [
              { name: "qty", shape: { kind: "Named", name: "Qty" }, pathSegment: "qty", default: null },
              { name: "note", shape: { kind: "Base", base: "String", json: "String", guards: [], expected: { kind: "Json", json: "String" } }, pathSegment: "note", default: "\"\"" },
            ],
          },
          loc: { uri: "file:///p/src/shop.bynk", range: range(2, 5) },
        },
        {
          name: "Qty",
          codecSuffix: "Qty",
          provenance: { kind: "Consumed", ownerUnit: "units" },
          body: { kind: "Scalar", base: "Int", json: "Number", opaque: false, predicates: ["Positive"], baseGuards: ["Integral"], revalidation: "ViaConstructor" },
          loc: null,
        },
      ],
      instantiations: [],
      recursive: [],
      responses: [
        { status: 201, variant: "Created", origin: { kind: "DeclaredSuccess" } },
        { status: 400, variant: "BadRequest", origin: { kind: "BoundaryImplicit", why: "the body failed validation" } },
      ],
      noCrossContext: null,
    };
    const out = renderWireContract(model, "file:///p/src/shop.bynk", rel);
    assert.ok(out.startsWith("# `shop` · service `api` · POST /orders (src/shop.bynk:10:3)\n"), out);
    assert.ok(out.includes("Request: `body: NewOrder`."), out);
    assert.ok(out.includes("- 201 `Created` (declared success)"), out);
    assert.ok(out.includes("- 400 `BadRequest` (added by the boundary: the body failed validation)"), out);
    assert.ok(out.includes("- `NewOrder` (src/shop.bynk:3:6): record\n  - `qty`: Qty\n  - `note`: String = \"\""), out);
    assert.ok(out.includes("- `Qty`, from `units`: Int where Positive, sent as JSON number"), out);
  });

  describe("renderSymbol", () => {
    it("joins hovers and lists definitions", () => {
      const out = renderSymbol("src/a.bynk:3:5", ["```bynk\ntype Qty = Int\n```"], ["src/a.bynk:1"]);
      assert.strictEqual(out, "# Symbol at src/a.bynk:3:5\n\n```bynk\ntype Qty = Int\n```\n\nDefined at: src/a.bynk:1\n");
    });

    it("says when nothing is known", () => {
      assert.match(renderSymbol("src/a.bynk:1:1", [], []), /^Nothing is known at src\/a\.bynk:1:1/);
    });
  });
});
