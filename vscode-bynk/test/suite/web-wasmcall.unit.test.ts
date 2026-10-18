// Unit coverage for the web worker's wasm calls (#2102): a failed analysis is
// reported as a failure, never as "no diagnostics", so the worker publishes
// nothing and the last good diagnostics stay.

import * as assert from "assert";

import { publishableDiagnostics } from "../../src/web/convert";
import { WasmCaller } from "../../src/web/wasmCall";

class Trap extends Error {}

function caller(load: () => Promise<void> = async () => {}) {
  const events: string[] = [];
  const wasm = new WasmCaller({
    load,
    onTrap: () => events.push("trap"),
    log: () => events.push("log"),
    isTrap: (err) => err instanceof Trap,
  });
  return { wasm, events };
}

const CLEAN = JSON.stringify({ diagnostics: [] });

describe("web WasmCaller", () => {
  it("returns the parsed reply", async () => {
    const { wasm } = caller();
    assert.deepStrictEqual(await wasm.call(() => CLEAN), { ok: true, value: { diagnostics: [] } });
  });

  it("reports a failed load, a throw or an unparsable reply as a failure", async () => {
    const failing = caller(() => Promise.reject(new Error("fetch failed")));
    assert.deepStrictEqual(await failing.wasm.call(() => CLEAN), { ok: false });
    const { wasm } = caller();
    assert.deepStrictEqual(
      await wasm.call(() => {
        throw new Error("boom");
      }),
      { ok: false },
    );
    assert.deepStrictEqual(await wasm.call(() => "not json"), { ok: false });
    // None of those poisons the instance.
    assert.ok(!wasm.isPoisoned);
  });

  it("never calls a trapped instance again, and reports the trap once", async () => {
    const { wasm, events } = caller();
    assert.deepStrictEqual(
      await wasm.call(() => {
        throw new Trap("unreachable");
      }),
      { ok: false },
    );
    let ran = false;
    const after = await wasm.call(() => {
      ran = true;
      return CLEAN;
    });
    assert.deepStrictEqual(after, { ok: false });
    assert.ok(!ran, "the poisoned instance is not called");
    assert.deepStrictEqual(events.filter((e) => e === "trap"), ["trap"]);
  });
});

describe("web publishableDiagnostics", () => {
  it("publishes nothing for a failed analysis", () => {
    assert.strictEqual(publishableDiagnostics({ ok: false }, "x", "untitled:x"), undefined);
  });

  it("publishes an empty set for a clean analysis", () => {
    const clean = publishableDiagnostics({ ok: true, value: { diagnostics: [] } }, "x", "untitled:x");
    assert.deepStrictEqual(clean, []);
  });
});
