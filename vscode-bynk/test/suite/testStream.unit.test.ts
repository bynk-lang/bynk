// Unit tests for the `bynk test --format ndjson` reader (#2267): the line
// buffering, the dropping of what is not an event, and the folding of events into
// the run document, one finished suite at a time.

import * as assert from "node:assert";
import { EventReader, RunAssembler, rejectsNdjson, type StreamEvent } from "../../src/testStream";

/** A run's stream as the driver writes it. */
const SAMPLE = [
  '{"type":"run-begin","suites":2}',
  '{"type":"suite-begin","name":"demo.m","kind":"unit","tests":2}',
  '{"type":"case","suite":"demo.m","name":"one","outcome":"pass","tier":"unit"}',
  '{"type":"case","suite":"demo.m","name":"never seventy three","outcome":"fail","message":"property failed","kind":"property","seed":"0x5f3a","counterexample":"p = 73"}',
  '{"type":"suite-end","name":"demo.m"}',
  '{"type":"suite-begin","name":"demo.n","kind":"unit","tests":1}',
  '{"type":"case","suite":"demo.n","name":"two","outcome":"pass","tier":"unit"}',
  '{"type":"suite-end","name":"demo.n"}',
  '{"type":"run-end","passed":2,"failed":1}',
];

function read(chunks: string[]): StreamEvent[] {
  const reader = new EventReader();
  return [...chunks.flatMap((c) => reader.push(c)), ...reader.end()];
}

describe("bynk test --format ndjson reader (#2267)", () => {
  describe("EventReader", () => {
    it("reads one event per line", () => {
      const events = read([SAMPLE.join("\n") + "\n"]);
      assert.deepStrictEqual(
        events.map((e) => e.type),
        ["run-begin", "suite-begin", "case", "case", "suite-end", "suite-begin", "case", "suite-end", "run-end"],
      );
    });

    it("does not corrupt an event split across chunks, at any boundary", () => {
      const text = SAMPLE.join("\n") + "\n";
      const whole = read([text]);
      for (let cut = 1; cut < text.length; cut += 7) {
        assert.deepStrictEqual(read([text.slice(0, cut), text.slice(cut)]), whole, `cut at ${cut}`);
      }
    });

    it("keeps a final line that has no newline until the stream ends", () => {
      const reader = new EventReader();
      assert.deepStrictEqual(reader.push('{"type":"run-end","passed":0,"failed":0}'), []);
      assert.strictEqual(reader.end().length, 1);
    });

    it("tolerates CRLF and blank lines, and drops what is not an event", () => {
      const events = read(['{"type":"run-begin","suites":0}\r\n\r\nnot json\r\n[1,2]\r\n{"x":1}\r\n{"type":3}\r\n']);
      assert.deepStrictEqual(events.map((e) => e.type), ["run-begin"]);
    });

    it("passes an event type it does not know through, for the caller to ignore", () => {
      assert.deepStrictEqual(read(['{"type":"progress","n":1}\n']).map((e) => e.type), ["progress"]);
    });
  });

  describe("RunAssembler", () => {
    function run(lines: string[]) {
      const assembler = new RunAssembler();
      const finished: string[] = [];
      for (const event of read([lines.join("\n") + "\n"])) {
        const done = assembler.apply(event);
        if (done) finished.push(done.name);
      }
      return { assembler, finished };
    }

    it("reports each case as it arrives, before its suite or the run has ended", () => {
      const assembler = new RunAssembler();
      const events = read([SAMPLE.join("\n") + "\n"]);
      const seen: string[] = [];
      for (const event of events) {
        const shown = assembler.apply(event);
        if (shown) seen.push(`${shown.name}/${shown.cases.map((c) => c.name).join(",")}@${event.type}`);
      }
      assert.deepStrictEqual(seen, [
        "demo.m/one@case",
        "demo.m/never seventy three@case",
        "demo.n/two@case",
      ]);
    });

    it("keeps a case's duration, which the Testing view shows", () => {
      const assembler = new RunAssembler();
      const shown: Array<number | undefined> = [];
      for (const event of read([
        [
          '{"type":"suite-begin","name":"s","kind":"unit","tests":1}',
          '{"type":"case","suite":"s","name":"a","outcome":"pass","durationMs":12.5}',
          '{"type":"case","suite":"s","name":"b","outcome":"skipped"}',
        ].join("\n") + "\n",
      ])) {
        const out = assembler.apply(event);
        if (out) shown.push(out.cases[0].durationMs);
      }
      assert.deepStrictEqual(shown, [12.5, undefined]);
    });

    it("builds the document the JSON run would have held, case fields intact", () => {
      const { assembler } = run(SAMPLE);
      const doc = assembler.finish("");
      assert.strictEqual(doc.passed, 2);
      assert.strictEqual(doc.failed, 1);
      assert.strictEqual(doc.error, undefined);
      const failed = doc.suites?.[0].cases[1];
      assert.deepStrictEqual(failed, {
        name: "never seventy three",
        outcome: "fail",
        message: "property failed",
        kind: "property",
        seed: "0x5f3a",
        counterexample: "p = 73",
      });
      assert.strictEqual(doc.suites?.[0].cases[0].tier, "unit");
    });

    it("keeps the declaration a violation or a refused step names (#2272)", () => {
      const violation = { kind: "invariant", agent: "Wallet", name: "nonneg", location: { path: "src/demo/wallet.bynk", line: 9, col: 13 } };
      const refusals = [{ index: 0, call: "Spend", ...violation }];
      const lines = [
        '{"type":"suite-begin","name":"demo.w","kind":"unit","tests":1}',
        JSON.stringify({ type: "case", suite: "demo.w", name: "c", outcome: "fail", message: "m", violation, refusals, durationMs: 1 }),
        '{"type":"suite-end","name":"demo.w"}',
        '{"type":"run-end","passed":0,"failed":1}',
      ];
      const failed = run(lines).assembler.finish("").suites?.[0].cases[0];
      assert.deepStrictEqual(failed?.violation, violation);
      assert.deepStrictEqual(failed?.refusals, refusals);
    });

    it("reports a skipped case that arrives after its suite on its own", () => {
      const assembler = new RunAssembler();
      const out: Array<{ name: string; cases: string[] }> = [];
      for (const event of read([
        [
          '{"type":"run-begin","suites":1}',
          '{"type":"suite-begin","name":"demo.s","kind":"integration","tests":1}',
          '{"type":"case","suite":"demo.s","name":"ran","outcome":"pass"}',
          '{"type":"suite-end","name":"demo.s"}',
          '{"type":"case","suite":"demo.s","name":"advances","outcome":"skipped","message":"advance Clock"}',
          '{"type":"run-end","passed":1,"failed":0,"skipped":1,"transport":"remote:staging"}',
        ].join("\n") + "\n",
      ])) {
        const done = assembler.apply(event);
        if (done) out.push({ name: done.name, cases: done.cases.map((c) => c.name) });
      }
      assert.deepStrictEqual(
        out,
        [
          { name: "demo.s", cases: ["ran"] },
          { name: "demo.s", cases: ["advances"] },
        ],
        "each case is shown as it arrives, the skipped one after its suite ended",
      );
      const doc = assembler.finish("");
      assert.strictEqual(doc.skipped, 1);
      assert.strictEqual(doc.transport, "remote:staging");
      assert.deepStrictEqual(
        doc.suites?.[0].cases.map((c) => c.name),
        ["ran", "advances"],
        "the document holds the skipped case in its suite",
      );
    });

    it("reads a compile failure from an error event", () => {
      const { assembler } = run(['{"type":"error","kind":"compile","diagnostics":["a.bynk:1:1: error[x]: no"]}']);
      const doc = assembler.finish("");
      assert.deepStrictEqual(doc.error, { kind: "compile", diagnostics: ["a.bynk:1:1: error[x]: no"] });
      assert.strictEqual(doc.suites?.length, 0);
    });

    it("keeps the suites a crashed run reached and reads its error event", () => {
      const { assembler } = run([
        '{"type":"run-begin","suites":2}',
        '{"type":"suite-begin","name":"demo.m","kind":"unit","tests":1}',
        '{"type":"case","suite":"demo.m","name":"one","outcome":"pass"}',
        '{"type":"suite-end","name":"demo.m"}',
        '{"type":"error","kind":"runtime","message":"the test runner exited before completing","stderr":"boom"}',
      ]);
      const doc = assembler.finish("");
      assert.strictEqual(doc.suites?.length, 1);
      assert.strictEqual(doc.error?.kind, "runtime");
      assert.strictEqual(doc.error?.stderr, "boom");
    });

    it("treats a stream with neither run-end nor error as a runtime failure carrying stderr", () => {
      const { assembler } = run(['{"type":"run-begin","suites":1}']);
      const doc = assembler.finish("Killed\n");
      assert.strictEqual(doc.error?.kind, "runtime");
      assert.strictEqual(doc.error?.stderr, "Killed");
      assert.ok(assembler.hasEvents);
    });

    it("has no events for a process that wrote none", () => {
      assert.strictEqual(new RunAssembler().hasEvents, false);
    });
  });

  describe("rejectsNdjson", () => {
    it("recognises a driver that does not know the format", () => {
      assert.ok(
        rejectsNdjson("error: invalid value 'ndjson' for '--format <FORMAT>'\n  [possible values: rich, json]\n"),
      );
    });

    it("does not mistake another failure for it", () => {
      assert.ok(!rejectsNdjson("bynk test: no `bynkc` compiler found"));
      assert.ok(!rejectsNdjson("error: invalid value 'x' for '--format <FORMAT>'"));
    });
  });
});
