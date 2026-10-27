// Unit coverage for the dev-server controller's pure core (#2094): the
// `bynk dev --format json` event stream, the state it drives, the terminal
// markers the `$bynk-dev` background matcher keys on, and the shared line
// splitter. The event lines below are the driver's own output, pinned by
// `bynk/src/dev.rs`'s `the_serving_event_reports_every_port` and
// `lifecycle_events_are_tagged_lines`; keep the two in step.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import { diagnosticPath, launchFailureAction, LineSplitter, parseShortDiagnostic, toTerminalText } from "../../src/cliCore";
import {
  contextNames,
  devArgs,
  devMarker,
  devStatus,
  DevState,
  DevTransition,
  INITIAL_DEV_STATE,
  parseDevEvent,
  PreflightWatch,
  reduceDevState,
} from "../../src/devEvents";

const SERVING =
  '{"event":"serving","contexts":[{"name":"commerce-orders","port":8787,"url":"http://localhost:8787","inspector_port":9229},{"name":"commerce-payment","port":8788,"url":"http://localhost:8788","inspector_port":9230}]}';

describe("parseDevEvent", () => {
  it("reads the driver's serving event", () => {
    assert.deepStrictEqual(parseDevEvent(SERVING), {
      event: "serving",
      contexts: [
        { name: "commerce-orders", port: 8787, url: "http://localhost:8787", inspectorPort: 9229 },
        { name: "commerce-payment", port: 8788, url: "http://localhost:8788", inspectorPort: 9230 },
      ],
    });
  });

  it("reads the lifecycle events", () => {
    assert.deepStrictEqual(parseDevEvent('{"event":"rebuilding"}'), { event: "rebuilding" });
    assert.deepStrictEqual(parseDevEvent('{"event":"rebuild_ok"}'), { event: "rebuild_ok" });
    assert.deepStrictEqual(parseDevEvent('{"event":"rebuild_failed"}'), { event: "rebuild_failed" });
    assert.deepStrictEqual(parseDevEvent('{"event":"stopped","code":130}'), { event: "stopped", code: 130 });
    // A missing code is unknown, not success.
    assert.deepStrictEqual(parseDevEvent('{"event":"stopped"}'), { event: "stopped", code: null });
  });

  it("tolerates unknown fields and a null inspector port", () => {
    assert.deepStrictEqual(
      parseDevEvent('{"event":"serving","future":1,"contexts":[{"name":"links","port":8787,"inspector_port":null,"extra":true}]}'),
      { event: "serving", contexts: [{ name: "links", port: 8787, url: "http://localhost:8787", inspectorPort: null }] },
    );
  });

  it("passes everything else through as plain output", () => {
    for (const line of [
      "",
      "bynk dev: watching for source changes",
      "{not json",
      '{"event":"something_new"}',
      '{"no_event":true}',
      '["serving"]',
      '{"event":"serving","contexts":[{"name":"x"}]}',
    ]) {
      assert.strictEqual(parseDevEvent(line), undefined, line);
    }
  });
});

describe("reduceDevState and devMarker", () => {
  /** Run transitions from the initial state, collecting the markers written. */
  function run(ts: DevTransition[]): { state: DevState; markers: string[] } {
    let state = INITIAL_DEV_STATE;
    const markers: string[] = [];
    for (const t of ts) {
      const m = devMarker(t, state);
      if (m) markers.push(m);
      state = reduceDevState(state, t);
    }
    return { state, markers };
  }

  it("walks a session through serve, ready, a failed and a good rebuild, and stop", () => {
    const serving = parseDevEvent(SERVING)!;
    const { state, markers } = run([
      serving,
      { event: "ready" },
      { event: "rebuilding" },
      { event: "rebuild_failed" },
      { event: "rebuilding" },
      { event: "rebuild_ok" },
      { event: "stopped", code: 0 },
      { event: "exited", code: 0 },
    ]);
    assert.strictEqual(state.phase, "stopped");
    assert.strictEqual(state.exitCode, 0);
    assert.deepStrictEqual(markers, [
      "[bynk dev] ready: commerce-orders http://localhost:8787, commerce-payment http://localhost:8788",
      "[bynk dev] rebuilding",
      "[bynk dev] rebuild failed (see Problems); the last good build is still serving",
      "[bynk dev] rebuilding",
      "[bynk dev] rebuilt",
      "[bynk dev] stopped (exit 0)",
    ]);
  });

  it("ends a session that never served, so a preLaunchTask does not hang", () => {
    const { state, markers } = run([{ event: "exited", code: 1 }]);
    assert.strictEqual(state.phase, "stopped");
    assert.deepStrictEqual(markers, ["[bynk dev] stopped (exit 1)"]);
  });

  it("keeps a rebuild during boot from ending the start-up cycle", () => {
    // serving → rebuilding → rebuild_ok → ready: the save lands while the
    // workers are still booting, so only the probe's `ready` ends the cycle.
    const steps: DevTransition[] = [parseDevEvent(SERVING)!, { event: "rebuilding" }, { event: "rebuild_ok" }];
    const booting = run(steps);
    assert.strictEqual(booting.state.phase, "serving");
    assert.strictEqual(booting.state.ready, false);
    assert.deepStrictEqual(booting.markers, [
      "[bynk dev] rebuilding",
      "[bynk dev] (starting) rebuilt; waiting for the workers to answer",
    ]);
    const { state, markers } = run([...steps, { event: "ready" }]);
    assert.strictEqual(state.phase, "ready");
    assert.strictEqual(state.ready, true);
    assert.strictEqual(
      markers[markers.length - 1],
      "[bynk dev] ready: commerce-orders http://localhost:8787, commerce-payment http://localhost:8788",
    );
  });

  it("keeps a failed boot rebuild from ending the cycle, and reports ready mid-rebuild", () => {
    const { state, markers } = run([
      parseDevEvent(SERVING)!,
      { event: "rebuilding" },
      { event: "rebuild_failed" },
      { event: "rebuilding" },
      { event: "ready" },
      { event: "rebuild_ok" },
    ]);
    assert.deepStrictEqual(markers, [
      "[bynk dev] rebuilding",
      "[bynk dev] (starting) rebuild failed (see Problems); waiting for the workers to answer",
      "[bynk dev] rebuilding",
      "[bynk dev] ready: commerce-orders http://localhost:8787, commerce-payment http://localhost:8788",
      "[bynk dev] rebuilt",
    ]);
    assert.strictEqual(state.phase, "ready");
  });

  it("ends the cycle when the readiness probe times out, without a spinner", () => {
    const { state, markers } = run([parseDevEvent(SERVING)!, { event: "probe_timeout" }, { event: "ready" }]);
    assert.strictEqual(state.phase, "ready");
    assert.strictEqual(state.unanswered, true);
    assert.deepStrictEqual(markers, [
      "[bynk dev] ready, but not every port answered within 120s: commerce-orders http://localhost:8787, commerce-payment http://localhost:8788",
    ]);
    const { text, tooltip } = devStatus(state);
    assert.ok(!text.includes("~spin"), text);
    assert.ok(text.startsWith("$(warning)"), text);
    assert.ok(tooltip.includes("not every port answered"), tooltip);
  });

  it("keeps the driver's exit code over a signal exit", () => {
    const { state } = run([{ event: "stopped", code: 143 }, { event: "exited", code: null }]);
    assert.strictEqual(state.exitCode, 143);
  });
});

describe("the $bynk-dev background patterns", () => {
  // The markers are only useful if package.json's matcher recognises them.
  const pkg = JSON.parse(fs.readFileSync(path.resolve(__dirname, "../../../package.json"), "utf8"));
  const matcher = pkg.contributes.problemMatchers.find((m: { name: string }) => m.name === "bynk-dev");
  const begins = new RegExp(matcher.background.beginsPattern);
  const ends = new RegExp(matcher.background.endsPattern);

  it("begins on start and rebuild, and ends on ready, rebuilt, failure, timeout and stop", () => {
    assert.ok(begins.test("[bynk dev] starting: bynk dev --format json"));
    assert.ok(begins.test("[bynk dev] rebuilding"));
    const serving = reduceDevState(INITIAL_DEV_STATE, parseDevEvent(SERVING)!);
    const ready = reduceDevState(serving, { event: "ready" });
    const ending: [DevTransition, DevState][] = [
      [{ event: "ready" }, serving],
      [{ event: "probe_timeout" }, serving],
      [{ event: "rebuild_ok" }, ready],
      [{ event: "rebuild_failed" }, ready],
      [{ event: "stopped", code: 0 }, serving],
    ];
    for (const [t, prev] of ending) {
      const m = devMarker(t, prev)!;
      assert.ok(ends.test(m), m);
      assert.ok(!begins.test(m), m);
    }
  });

  it("does not end the start-up cycle on a rebuild before the workers answer", () => {
    const serving = reduceDevState(INITIAL_DEV_STATE, parseDevEvent(SERVING)!);
    for (const t of [{ event: "rebuild_ok" }, { event: "rebuild_failed" }] as DevTransition[]) {
      const m = devMarker(t, serving)!;
      assert.ok(!ends.test(m), m);
      assert.ok(!begins.test(m), m);
    }
  });
});

describe("devStatus", () => {
  it("names the context count and lists the URLs", () => {
    const s = reduceDevState(reduceDevState(INITIAL_DEV_STATE, parseDevEvent(SERVING)!), { event: "ready" });
    const { text, tooltip } = devStatus(s);
    assert.strictEqual(text, "$(radio-tower) dev: 2 contexts");
    assert.ok(tooltip.includes("commerce-payment: http://localhost:8788"), tooltip);
  });

  it("says one context in the singular", () => {
    const s = reduceDevState(INITIAL_DEV_STATE, parseDevEvent('{"event":"serving","contexts":[{"name":"links","port":8787}]}')!);
    assert.strictEqual(devStatus(s).text, "$(loading~spin) dev: 1 context");
  });
});

describe("devArgs", () => {
  it("always asks for the event stream", () => {
    assert.deepStrictEqual(devArgs({}), ["dev", "--format", "json", "--log-events"]);
  });

  it("adds contexts, the base port and the wrangler passthrough", () => {
    assert.deepStrictEqual(
      devArgs({ contexts: ["commerce.orders"], basePort: 9000, extraArgs: ["--var", "K:V"] }),
      ["dev", "--format", "json", "--log-events", "--context", "commerce.orders", "--base-port", "9000", "--", "--var", "K:V"],
    );
    assert.deepStrictEqual(devArgs({ basePort: null, extraArgs: [] }), ["dev", "--format", "json", "--log-events"]);
  });
});

describe("contextNames", () => {
  it("finds context declarations, dotted or not, and nothing else", () => {
    const src = "context commerce.orders\n\n// context not.this\nservice Api from http {}\n";
    assert.deepStrictEqual(contextNames(src), ["commerce.orders"]);
    assert.deepStrictEqual(contextNames("commons fixture.text\n"), []);
  });
});

describe("LineSplitter", () => {
  it("joins lines split across chunks, including a split CRLF", () => {
    const s = new LineSplitter();
    assert.deepStrictEqual(s.push('{"event":"reb'), []);
    assert.deepStrictEqual(s.push('uilding"}\r'), []);
    assert.deepStrictEqual(s.push('\n{"event":"rebuild_ok"}\npartial'), [
      '{"event":"rebuilding"}',
      '{"event":"rebuild_ok"}',
    ]);
    assert.deepStrictEqual(s.flush(), ["partial"]);
    assert.deepStrictEqual(s.flush(), []);
  });
});

describe("parseShortDiagnostic and toTerminalText", () => {
  it("parses the `--format short` shape", () => {
    assert.deepStrictEqual(
      parseShortDiagnostic("src/orders.bynk:3:7: error[bynk.resolve.unknown_type]: unknown type `Ordr`"),
      {
        file: "src/orders.bynk",
        line: 3,
        column: 7,
        severity: "error",
        code: "bynk.resolve.unknown_type",
        message: "unknown type `Ordr`",
      },
    );
    assert.strictEqual(parseShortDiagnostic("bynk dev: rebuilt"), undefined);
  });

  it("resolves a diagnostic's file, relative or absolute", () => {
    const root = path.resolve("/work/shop");
    assert.strictEqual(diagnosticPath(root, "src/a.bynk"), path.join(root, "src", "a.bynk"));
    const abs = path.resolve("/elsewhere/src/b.bynk");
    assert.strictEqual(diagnosticPath(root, abs), abs);
  });

  it("gives the pseudoterminal CRLF line ends", () => {
    assert.strictEqual(toTerminalText("a\nb\r\nc"), "a\r\nb\r\nc");
  });
});

describe("PreflightWatch", () => {
  it("sees the pre-flight refusal in one chunk", () => {
    const w = new PreflightWatch();
    assert.strictEqual(w.push("bynk: environment not ready for `dev` — see below.\n"), true);
  });

  it("sees it split across chunks, even one character at a time", () => {
    const line = "bynk: environment not ready for `dev` — see below.\n";
    const split = new PreflightWatch();
    assert.strictEqual(split.push("bynk: environment not re"), false);
    assert.strictEqual(split.push("ady for `dev` — see below.\n"), true);
    const byChar = new PreflightWatch();
    let seen = false;
    for (const ch of line) seen = byChar.push(ch);
    assert.strictEqual(seen, true);
  });

  it("stays quiet on ordinary output, and stays set once seen", () => {
    const w = new PreflightWatch();
    assert.strictEqual(w.push("bynk dev: serving 2 contexts\n"), false);
    assert.strictEqual(w.push("environment not ready for `dev`"), true);
    assert.strictEqual(w.push("anything"), true);
  });
});

describe("launchFailureAction", () => {
  it("offers the install message only for a spawn whose code is ENOENT", () => {
    const spawnError = Object.assign(new Error("spawn bynk ENOENT"), { code: "ENOENT" });
    assert.strictEqual(launchFailureAction(spawnError), "install");
  });

  it("offers the Doctor when the debuggee's stderr merely mentions ENOENT", () => {
    const exited = new Error(
      "the inspector process exited before it was ready\nError: ENOENT: no such file or directory, open 'tsconfig.json'",
    );
    assert.strictEqual(launchFailureAction(exited), "doctor");
    assert.strictEqual(launchFailureAction("ENOENT"), "doctor");
    assert.strictEqual(launchFailureAction(undefined), "doctor");
    assert.strictEqual(launchFailureAction(Object.assign(new Error("x"), { code: "EACCES" })), "doctor");
  });
});

describe("parseDevEvent: log (#2474)", () => {
  it("reads a worker log line", () => {
    assert.deepStrictEqual(
      parseDevEvent('{"event":"log","context":"shop","level":"error","text":"    at f (/p/src/a.bynk:3:1)"}'),
      { event: "log", context: "shop", level: "error", text: "    at f (/p/src/a.bynk:3:1)" },
    );
  });

  it("takes an unknown level as info and refuses a line without its parts", () => {
    assert.deepStrictEqual(
      parseDevEvent('{"event":"log","context":"shop","level":"trace","text":"x"}'),
      { event: "log", context: "shop", level: "info", text: "x" },
    );
    assert.strictEqual(parseDevEvent('{"event":"log","context":"shop"}'), undefined);
    assert.strictEqual(parseDevEvent('{"event":"log","text":"x"}'), undefined);
  });

  it("is not a lifecycle change", () => {
    const serving = reduceDevState(INITIAL_DEV_STATE, parseDevEvent(SERVING)!);
    const log: DevTransition = { event: "log", context: "shop", level: "info", text: "x" };
    assert.strictEqual(reduceDevState(serving, log), serving);
    assert.strictEqual(devMarker(log, serving), undefined);
  });
});
