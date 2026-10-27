// #2469: unit coverage for `capabilityPathCore` — the validation of the
// arguments the server's "Show capability path" action sends.

import * as assert from "assert";

import { parseArguments } from "../../src/capabilityPathCore";

const POS = { line: 3, character: 4 };
const LOC = {
  uri: "file:///shop/files.bynk",
  range: { start: { line: 15, character: 4 }, end: { line: 15, character: 24 } },
};

describe("capability-path arguments", () => {
  it("accepts the shape the server sends", () => {
    const args = parseArguments("file:///shop/takedown.bynk", POS, [LOC, LOC]);
    assert.deepStrictEqual(args, {
      uri: "file:///shop/takedown.bynk",
      position: POS,
      locations: [LOC, LOC],
    });
  });

  it("accepts an empty location list", () => {
    assert.ok(parseArguments("file:///a.bynk", POS, []));
  });

  it("rejects missing or malformed arguments", () => {
    assert.strictEqual(parseArguments(undefined, POS, [LOC]), undefined);
    assert.strictEqual(parseArguments("file:///a.bynk", "3:4", [LOC]), undefined);
    assert.strictEqual(parseArguments("file:///a.bynk", POS, "nope"), undefined);
    assert.strictEqual(parseArguments("file:///a.bynk", POS, [{ ...LOC, uri: 7 }]), undefined);
    assert.strictEqual(
      parseArguments("file:///a.bynk", POS, [{ uri: LOC.uri, range: { start: POS } }]),
      undefined,
    );
  });
});
