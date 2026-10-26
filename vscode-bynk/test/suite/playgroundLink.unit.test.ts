// #2480: the playground deep link — encoding and the size cap. Pure (no
// `vscode`), so it runs under plain Mocha.

import * as assert from "assert";
import { randomBytes } from "crypto";
import { inflateRawSync } from "zlib";

import {
  decodeFragment,
  encodeFragment,
  foreignReferences,
  MAX_LINK_LENGTH,
  PLAYGROUND_ORIGIN,
  playgroundLink,
  usedUnits,
} from "../../src/playgroundLink";

const SNIPPET = "type OrderId = opaque String\n\nfn parse(raw: String) -> Result[OrderId, ValidationError] {\n  OrderId.of(raw)\n}\n";

describe("Playground link (#2480)", () => {
  it("opens the production playground with the code in the fragment", async () => {
    const link = await playgroundLink(SNIPPET);
    assert.ok(link.ok);
    assert.ok(link.url.startsWith(`${PLAYGROUND_ORIGIN}/#`));
  });

  it("is base64url (no padding or characters needing escape) of raw deflate of the UTF-8", async () => {
    const fragment = await encodeFragment(SNIPPET);
    assert.match(fragment, /^[A-Za-z0-9_-]+$/);
    const bytes = Buffer.from(fragment, "base64url");
    assert.strictEqual(inflateRawSync(bytes).toString("utf8"), SNIPPET);
  });

  it("round-trips, including non-ASCII text", async () => {
    for (const source of [SNIPPET, "", "// café — 日本語 🎉\n", "a".repeat(5000)]) {
      assert.strictEqual(await decodeFragment(await encodeFragment(source)), source);
    }
  });

  it("refuses a snippet whose link passes the cap, rather than truncating it", async () => {
    const incompressible = randomBytes(MAX_LINK_LENGTH).toString("base64");
    const link = await playgroundLink(incompressible);
    assert.ok(!link.ok);
    assert.strictEqual(link.max, MAX_LINK_LENGTH);
    assert.ok(link.length > MAX_LINK_LENGTH);
  });

  it("allows a link of exactly the cap and refuses one char over", async () => {
    const url = (await playgroundLink(SNIPPET)) as { ok: true; url: string };
    assert.ok((await playgroundLink(SNIPPET, url.url.length)).ok);
    assert.ok(!(await playgroundLink(SNIPPET, url.url.length - 1)).ok);
  });

  it("admits a large but compressible program", async () => {
    const big = SNIPPET.repeat(200);
    assert.ok(big.length > MAX_LINK_LENGTH);
    assert.ok((await playgroundLink(big)).ok);
  });
});

describe("Cross-file references in a selection (#2480, DECISION B)", () => {
  const file = "commons blocks\n\nuses inline\nuses keys\n\nfn a() { inline.render(x) }\nfn b() { 1 }\n";

  it("reads the units a file uses", () => {
    assert.deepStrictEqual(usedUnits(file), ["inline", "keys"]);
  });

  it("flags a selection that names a used unit", () => {
    assert.deepStrictEqual(foreignReferences(file, "fn a() { inline.render(x) }"), ["inline"]);
  });

  it("is quiet for a selection that stands alone", () => {
    assert.deepStrictEqual(foreignReferences(file, "fn b() { 1 }"), []);
  });

  it("is quiet when the selection carries its own `uses`", () => {
    assert.deepStrictEqual(foreignReferences(file, "uses inline\nfn a() { inline.render(x) }"), []);
  });

  it("does not match a longer name or a member that merely shares the word", () => {
    assert.deepStrictEqual(foreignReferences(file, "fn c() { inlined(1) + x.inline }"), []);
  });
});
