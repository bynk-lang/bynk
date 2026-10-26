// Unit tests for the Bynk Logs view's core (#2474): the bounded buffer, the
// filter, and how an entry is rendered. The view itself is covered by
// dev_server.test.ts against the stand-in `bynk`.

import * as assert from "assert";

import { describeFilter, LogBuffer, LogEntry, NO_FILTER, passes, renderEntry } from "../../src/devLogs";

const entry = (context: string, level: LogEntry["level"], text: string): LogEntry => ({ context, level, text });

describe("LogBuffer", () => {
  it("keeps the newest entries within its capacity", () => {
    const buffer = new LogBuffer(10);
    for (let i = 0; i < 100; i++) buffer.push(entry("a", "info", `line ${i}`));
    const kept = buffer.view().map((e) => e.text);
    assert.strictEqual(kept.length, 10);
    assert.strictEqual(kept[0], "line 90");
    assert.strictEqual(kept[9], "line 99");
    assert.strictEqual(buffer.size, 10);
  });

  it("holds no more than a tenth over its capacity, however many arrive", () => {
    const buffer = new LogBuffer(100);
    let most = 0;
    for (let i = 0; i < 10_000; i++) {
      buffer.push(entry("a", "info", String(i)));
      most = Math.max(most, (buffer as unknown as { entries: unknown[] }).entries.length);
    }
    assert.ok(most <= 110, `held ${most}`);
  });

  it("clears, and lists the contexts it has seen", () => {
    const buffer = new LogBuffer(10);
    buffer.push(entry("shop", "info", "x"));
    buffer.push(entry("billing", "info", "y"));
    assert.deepStrictEqual(buffer.contexts(), ["billing", "shop"]);
    buffer.clear();
    assert.deepStrictEqual(buffer.view(), []);
    assert.deepStrictEqual(buffer.contexts(), []);
  });
});

describe("log filter", () => {
  const info = entry("shop", "info", "GET / 200");
  const warn = entry("shop", "warn", "slow");
  const error = entry("billing", "error", "boom");

  it("shows everything by default", () => {
    assert.deepStrictEqual([info, warn, error].filter((e) => passes(NO_FILTER, e)), [info, warn, error]);
  });

  it("filters by level, as a minimum", () => {
    assert.deepStrictEqual([info, warn, error].filter((e) => passes({ minLevel: "warn" }, e)), [warn, error]);
    assert.deepStrictEqual([info, warn, error].filter((e) => passes({ minLevel: "error" }, e)), [error]);
  });

  it("filters by context, together with level", () => {
    const filter = { minLevel: "warn" as const, contexts: new Set(["shop"]) };
    assert.deepStrictEqual([info, warn, error].filter((e) => passes(filter, e)), [warn]);
  });

  it("views through a filter", () => {
    const buffer = new LogBuffer(10);
    for (const e of [info, warn, error]) buffer.push(e);
    assert.deepStrictEqual(buffer.view({ minLevel: "error" }), [error]);
  });

  it("describes itself", () => {
    assert.strictEqual(describeFilter(NO_FILTER), "all logs");
    assert.strictEqual(
      describeFilter({ minLevel: "error", contexts: new Set(["b", "a"]) }),
      "error and above; a, b",
    );
  });
});

describe("renderEntry", () => {
  it("tags a line with its context and keeps a frame's indent and its .bynk link intact", () => {
    const frame = entry("shop", "error", "    at http_GET (/work/src/shop.bynk:12:3)");
    assert.strictEqual(renderEntry(frame, true), "[shop]     at http_GET (/work/src/shop.bynk:12:3)");
    assert.strictEqual(renderEntry(frame, false), frame.text);
  });
});
