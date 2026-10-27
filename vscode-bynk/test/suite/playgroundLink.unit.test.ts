// #2480: the playground deep link — encoding and the size cap. Pure (no
// `vscode`), so it runs under plain Mocha.

import * as assert from "assert";
import { randomBytes } from "crypto";
import { readFileSync } from "fs";
import { resolve } from "path";
import { inflateRawSync } from "zlib";

import {
  decodeFragment,
  encodeFragment,
  MAX_LINK_LENGTH,
  missingUses,
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

describe("Cross-file names in a selection (#2480, DECISION B)", () => {
  // examples/hello-world/src/hello/web.bynk: `uses` brings `greeting` and
  // `Subject` in unqualified, so the service block names nothing of `hello.text`.
  const web = readFileSync(
    resolve(__dirname, "../../../../examples/hello-world/src/hello/web.bynk"),
    "utf8",
  );
  const service = web.slice(web.indexOf("service api"));

  it("reads the units a file uses", () => {
    assert.deepStrictEqual(usedUnits(web), ["hello.text"]);
  });

  it("flags a selection that leaves out the file's `uses`", () => {
    assert.ok(service.includes("greeting("));
    assert.deepStrictEqual(missingUses(web, service), ["hello.text"]);
  });

  it("is quiet when the selection carries the `uses`", () => {
    assert.deepStrictEqual(missingUses(web, web.slice(web.indexOf("uses hello.text"))), []);
  });

  it("is quiet for a file that uses nothing", () => {
    assert.deepStrictEqual(missingUses("commons a\n\nfn b() { 1 }\n", "fn b() { 1 }"), []);
  });

  it("names only the units left out", () => {
    const file = "commons x\n\nuses a\nuses b.c\n\nfn f() { g() }\n";
    assert.deepStrictEqual(missingUses(file, "uses a\nfn f() { g() }"), ["b.c"]);
  });
});
