// The architecture map feeds theme colours into a Mermaid `classDef`, where a
// comma ends the property. See src/webview/classdef-color.ts.

import * as assert from "assert";

import { toClassDefColor } from "../../src/webview/classdef-color";

describe("toClassDefColor", () => {
  it("turns an rgba() with commas into comma-free hex", () => {
    assert.strictEqual(toClassDefColor("rgba(38, 79, 120, 0.5)"), "#264f7880");
  });
  it("drops the alpha byte when opaque", () => {
    assert.strictEqual(toClassDefColor("rgb(30, 30, 30)"), "#1e1e1e");
    assert.strictEqual(toClassDefColor("rgba(30, 30, 30, 1)"), "#1e1e1e");
  });
  it("reads the space-separated form", () => {
    assert.strictEqual(toClassDefColor("rgb(38 79 120 / 50%)"), "#264f7880");
  });
  it("yields no comma for any parsed colour", () => {
    assert.ok(!toClassDefColor("rgba(1, 2, 3, 0.25)")!.includes(","));
  });
  it("returns null for anything else", () => {
    assert.strictEqual(toClassDefColor("color(srgb 0.1 0.2 0.3)"), null);
    assert.strictEqual(toClassDefColor(""), null);
  });
});
