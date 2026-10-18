// #2097: the project sidebar's tree construction, from a pinned
// architecture-model fixture (`test/fixtures/architecture-model.json`) plus
// unit headers, without a live server.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import { buildProjectModel, parseUnitHeader, type UnitHeader } from "../../src/projectModel";
import type { ArchModel } from "../../src/webview/architectureTypes";

// Compiled to out/test/suite; the fixture stays in test/fixtures.
const FIXTURE = path.resolve(__dirname, "../../../test/fixtures/architecture-model.json");
const ARCH: ArchModel = JSON.parse(fs.readFileSync(FIXTURE, "utf8"));

const h = (kind: UnitHeader["kind"], name: string, uri: string): UnitHeader => ({
  kind,
  name,
  uri,
  line: 0,
  character: kind === "test" ? 6 : kind.length + 1,
});

// What the header scan finds: `storefront` spans two files, plus a commons
// and a test suite the architecture model does not cover.
const HEADERS: UnitHeader[] = [
  h("context", "billing", "file:///p/src/billing.bynk"),
  h("context", "storefront", "file:///p/src/storefront/web.bynk"),
  h("context", "storefront", "file:///p/src/storefront/cart.bynk"),
  h("adapter", "payments", "file:///p/src/payments.bynk"),
  h("commons", "money", "file:///p/src/money.bynk"),
  h("test", "billing", "file:///p/tests/billing.bynk"),
];

describe("project tree model (#2097)", () => {
  const groups = buildProjectModel(HEADERS, ARCH);
  const unit = (kind: string, name: string) => {
    const u = groups.find((g) => g.kind === kind)?.units.find((x) => x.name === name);
    assert.ok(u, `${kind} ${name} is in the tree`);
    return u;
  };

  it("groups units by kind, in context / commons / adapter / test order", () => {
    assert.deepStrictEqual(
      groups.map((g) => [g.label, g.units.map((u) => u.name)]),
      [
        ["Contexts", ["billing", "storefront"]],
        ["Commons", ["money"]],
        ["Adapters", ["payments"]],
        ["Tests", ["billing"]],
      ],
    );
  });

  it("gives each unit its consumes edges in both directions (DECISION C)", () => {
    const storefront = unit("context", "storefront");
    assert.deepStrictEqual(
      storefront.consumes.map((e) => [e.unit, e.capabilities]),
      [
        ["billing", []],
        ["bynk.cloudflare", ["Kv"]],
        ["payments", ["Charge"]],
      ],
    );
    assert.deepStrictEqual(storefront.consumedBy, []);
    assert.deepStrictEqual(
      unit("adapter", "payments").consumedBy.map((e) => e.unit),
      ["billing", "storefront"],
    );
    // An edge opens the consuming unit's `consumes` clause.
    assert.strictEqual(unit("context", "billing").consumedBy[0].loc.uri, "file:///p/src/storefront/web.bynk");
  });

  it("keeps a multi-file unit's files and opens it at the model's header", () => {
    const storefront = unit("context", "storefront");
    assert.deepStrictEqual(storefront.files, [
      "file:///p/src/storefront/cart.bynk",
      "file:///p/src/storefront/web.bynk",
    ]);
    assert.strictEqual(storefront.loc?.uri, "file:///p/src/storefront/web.bynk");
    assert.ok(storefront.arch, "carries its architecture node");
  });

  it("lists units the model lacks from their headers, and works with no model", () => {
    assert.strictEqual(unit("commons", "money").arch, undefined);
    assert.strictEqual(unit("commons", "money").loc?.uri, "file:///p/src/money.bynk");
    const bare = buildProjectModel(HEADERS, null);
    assert.deepStrictEqual(
      bare.map((g) => g.units.length),
      [2, 1, 1, 1],
      "a cold start (no model yet) still lists every unit",
    );
    assert.ok(bare.every((g) => g.units.every((u) => u.consumes.length === 0)), "but no edges");
  });
});

describe("unit header scan (#2097)", () => {
  it("reads each unit kind's header, a suite as a test", () => {
    assert.deepStrictEqual(parseUnitHeader("context shop\n\nservice api {}\n"), {
      kind: "context",
      name: "shop",
      line: 0,
      character: 8,
    });
    assert.strictEqual(parseUnitHeader("commons money.fmt\n")?.name, "money.fmt");
    assert.strictEqual(parseUnitHeader("adapter payments\n")?.kind, "adapter");
    assert.deepStrictEqual(parseUnitHeader('suite calc {\n  case "x" {}\n}\n'), {
      kind: "test",
      name: "calc",
      line: 0,
      character: 6,
    });
  });

  it("skips leading comments and doc blocks, including a header-shaped doc line", () => {
    const src = "-- a comment\n---\nThe context shop does things.\ncontext fake\n---\ncontext real\n";
    assert.deepStrictEqual(parseUnitHeader(src), { kind: "context", name: "real", line: 5, character: 8 });
  });

  it("returns undefined for a file with no header", () => {
    assert.strictEqual(parseUnitHeader(""), undefined);
    assert.strictEqual(parseUnitHeader("fn f() -> Int { 1 }\n"), undefined);
  });
});
