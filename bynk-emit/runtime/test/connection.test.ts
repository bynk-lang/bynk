// #3051: the case-side `TestConnection` — what `svc.open(tc, …)` remembers, and
// the one-owner rule that stops a handle being opened and handed out too.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  BEARER_SUBPROTOCOL,
  TestConnection,
  WorkersConnection,
  closeArguments,
  webSocketAuthExpired,
  webSocketClosing,
  webSocketErrored,
  webSocketBearerOffer,
  webSocketBearerProtocol,
  webSocketBearerRefusal,
  webSocketUpgradeResponse,
} from "../src/connection.ts";

test("message/close before open fail the case", () => {
  const tc = new TestConnection<string>();
  assert.throws(() => tc.opened(), /before `open`/);
});

test("open remembers the route values and deps", () => {
  const tc = new TestConnection<string>();
  tc.remember(["room-1"], { identity: "alice" });
  assert.deepEqual(tc.opened(), { route: ["room-1"], deps: { identity: "alice" } });
});

test("a connection opens once", () => {
  const tc = new TestConnection<string>();
  tc.remember([], {});
  assert.throws(() => tc.remember([], {}), /already opened/);
});

test("a connection has one owner: connection() once, and not after open", () => {
  const a = new TestConnection<string>();
  a.mint();
  assert.throws(() => a.mint(), /one socket has one owner/);
  assert.throws(() => a.remember([], {}), /already opened or handed/);
  const b = new TestConnection<string>();
  b.remember([], {});
  assert.throws(() => b.mint(), /already handed out or opened/);
});

test("sent frames are recorded and a closed connection refuses more", async () => {
  const tc = new TestConnection<string>();
  await tc.send("a");
  await tc.close();
  assert.deepEqual(tc.sent, ["a"]);
  assert.equal(tc.closed, true);
  await assert.rejects(tc.send("b"), /closed/);
});

// #3900: the Bearer handshake offers `["bynk.bearer", token]` and the 101 selects `bynk.bearer`.
test("a Bearer offer is the token after the bynk.bearer marker", () => {
  assert.deepEqual(webSocketBearerOffer("bynk.bearer, tok.en.sig"), { tag: "Token", token: "tok.en.sig" });
  assert.deepEqual(webSocketBearerOffer("bynk.bearer,tok"), { tag: "Token", token: "tok" });
  // The marker need not come first; the token is the element after it.
  assert.deepEqual(webSocketBearerOffer("other, bynk.bearer, tok"), { tag: "Token", token: "tok" });
});

test("no offer, an empty one, or a marker with no token is absent", () => {
  assert.deepEqual(webSocketBearerOffer(null), { tag: "Absent" });
  assert.deepEqual(webSocketBearerOffer(""), { tag: "Absent" });
  assert.deepEqual(webSocketBearerOffer(" , "), { tag: "Absent" });
  assert.deepEqual(webSocketBearerOffer("bynk.bearer"), { tag: "Absent" });
});

test("the old single-element form is legacy, never a token", () => {
  assert.deepEqual(webSocketBearerOffer("tok.en.sig"), { tag: "Legacy" });
  assert.deepEqual(webSocketBearerOffer("a, b"), { tag: "Legacy" });
});

test("the response selects the marker only when the client offered it", () => {
  assert.equal(webSocketBearerProtocol("bynk.bearer, tok"), BEARER_SUBPROTOCOL);
  assert.equal(webSocketBearerProtocol("bynk.bearer"), BEARER_SUBPROTOCOL);
  assert.equal(webSocketBearerProtocol("tok"), undefined);
  assert.equal(webSocketBearerProtocol(null), undefined);
});

test("the refusal of the old form is a 400 with a fixed message that never echoes the client", async () => {
  const res = webSocketBearerRefusal();
  assert.equal(res.status, 400);
  assert.equal(await res.text(), 'Offer the subprotocols ["bynk.bearer", <token>]');
});

test("the 101 carries the selected subprotocol, and none when none is selected", () => {
  // Node's Response forbids a 101, so build the init the helper builds and read its header.
  const seen: { status?: number; headers?: Record<string, string> }[] = [];
  const RealResponse = globalThis.Response;
  globalThis.Response = class {
    constructor(_body: unknown, init: { status?: number; headers?: Record<string, string> }) {
      seen.push(init);
    }
  } as unknown as typeof Response;
  try {
    webSocketUpgradeResponse({} as never, "bynk.bearer");
    webSocketUpgradeResponse({} as never);
  } finally {
    globalThis.Response = RealResponse;
  }
  assert.equal(seen[0].status, 101);
  assert.deepEqual(seen[0].headers, { "Sec-WebSocket-Protocol": "bynk.bearer" });
  assert.equal(seen[1].status, 101);
  assert.equal(seen[1].headers, undefined);
});

// #3902: close(code?, reason?), and the close handshake the Durable Object completes.
test("a TestConnection records the code and reason it was closed with", async () => {
  const tc = new TestConnection<string>();
  await tc.close(4001, "room archived");
  assert.equal(tc.closed, true);
  assert.equal(tc.closeCode, 4001);
  assert.equal(tc.closeReason, "room archived");
  assert.deepEqual(tc.closeCodeOption(), { tag: "Some", value: 4001 });
  assert.deepEqual(tc.closeReasonOption(), { tag: "Some", value: "room archived" });
  const bare = new TestConnection<string>();
  await bare.close();
  assert.equal(bare.closeCode, undefined);
  assert.equal(bare.closeCodeOption().tag, "None");
  assert.equal(bare.closeReasonOption().tag, "None");
});

test("a computed close code outside 1000 and 3000-4999 faults; a long reason is cut on a character boundary", () => {
  for (const bad of [999, 1001, 1004, 1005, 1006, 1015, 2999, 5000, 1000.5]) {
    assert.throws(() => closeArguments(bad), /not a close code/, String(bad));
  }
  assert.deepEqual(closeArguments(1000), { code: 1000 });
  assert.deepEqual(closeArguments(3000, "ok"), { code: 3000, reason: "ok" });
  // 40 three-byte characters are 120 bytes; 41 are 123; 42 are 126 and are cut to 41.
  const cut = closeArguments(4000, "€".repeat(42));
  assert.equal(cut.reason, "€".repeat(41));
  assert.ok(new TextEncoder().encode(cut.reason).length <= 123);
});

test("a WorkersConnection closes with exactly the arguments it was given", async () => {
  const calls: unknown[][] = [];
  const ws = { send: () => {}, close: (...a: unknown[]) => void calls.push(a) };
  const c = new WorkersConnection<string>(ws, "id");
  await c.close();
  await c.close(4001);
  await c.close(4001, "bye");
  assert.deepEqual(calls, [[], [4001], [4001, "bye"]]);
});

function socket() {
  const closes: unknown[][] = [];
  const attachments: unknown[] = [];
  return {
    closes,
    attachments,
    ws: {
      close: (...a: unknown[]) => void closes.push(a),
      serializeAttachment: (v: unknown) => void attachments.push(v),
    },
  };
}

test("webSocketClosing runs the handler with the peer's details, marks the socket, then echoes the code", async () => {
  const s = socket();
  const seen: unknown[] = [];
  await webSocketClosing(s.ws, { connId: "c" }, { code: 4001, reason: "bye", clean: true }, async (i) => void seen.push(i));
  assert.deepEqual(seen, [{ code: 4001, reason: "bye", clean: true }]);
  assert.deepEqual(s.attachments, [{ connId: "c", closed: true }]);
  assert.deepEqual(s.closes, [[4001, "bye"]]);
});

test("a close code that cannot be sent is acknowledged with a bare close()", async () => {
  for (const code of [1005, 1006, 1004, 1015, 0, 5000]) {
    const s = socket();
    await webSocketClosing(s.ws, { connId: "c" }, { code, reason: "", clean: false }, async () => {});
    assert.deepEqual(s.closes, [[]], String(code));
  }
});

test("the handshake completes after a faulting handler, and the fault still propagates", async () => {
  const s = socket();
  await assert.rejects(
    webSocketClosing(s.ws, { connId: "c" }, { code: 1000, reason: "", clean: true }, async () => {
      throw new Error("boom");
    }),
    /boom/,
  );
  assert.deepEqual(s.closes, [[1000, ""]]);
});

test("a socket already closed by an error replies but does not run the handler again", async () => {
  const s = socket();
  let runs = 0;
  const run = async () => void (runs += 1);
  const errors: unknown[] = [];
  const log = console.error;
  console.error = (...a: unknown[]) => void errors.push(a);
  try {
    const att = { connId: "c" };
    await webSocketErrored(s.ws, att, new TypeError("secret frame text"), run);
    assert.equal(runs, 1);
    assert.deepEqual(s.closes, [[1011, "error"]]);
    // The platform then delivers the close for the same socket, whose attachment is now marked.
    await webSocketClosing(s.ws, s.attachments[0], { code: 1011, reason: "error", clean: false }, run);
    assert.equal(runs, 1);
  } finally {
    console.error = log;
  }
  assert.deepEqual(errors, [["websocket error: TypeError"]]);
  assert.ok(!JSON.stringify(errors).includes("secret frame text"));
});

test("an error reports the info on_close sees as code 1011, reason error, not clean", async () => {
  const s = socket();
  const seen: unknown[] = [];
  const log = console.error;
  console.error = () => {};
  try {
    await webSocketErrored(s.ws, { connId: "c" }, "nope", async (i) => void seen.push(i));
  } finally {
    console.error = log;
  }
  assert.deepEqual(seen, [{ code: 1011, reason: "error", clean: false }]);
});

// #3903: a connection whose credential has expired is closed at its next frame.
test("a credential is expired once its exp is in the past, and not before", () => {
  assert.equal(webSocketAuthExpired({ exp: 100 }, 99), false);
  assert.equal(webSocketAuthExpired({ exp: 100 }, 100), false, "the HTTP Bearer bound is exp < now");
  assert.equal(webSocketAuthExpired({ exp: 100 }, 101), true);
});

test("a connection with no exp never expires", () => {
  assert.equal(webSocketAuthExpired({ connId: "c" }, 1e12), false);
  assert.equal(webSocketAuthExpired({ exp: undefined }, 1e12), false);
  assert.equal(webSocketAuthExpired({ exp: "soon" }, 1e12), false, "a non-number exp is not an expiry");
  assert.equal(webSocketAuthExpired(null, 1e12), false);
});

test("the default clock is the current time in seconds", () => {
  const now = Date.now() / 1000;
  assert.equal(webSocketAuthExpired({ exp: now - 60 }), true);
  assert.equal(webSocketAuthExpired({ exp: now + 60 }), false);
});

test("the default clock reads whole seconds, as the HTTP Bearer check does", () => {
  // At 100.5s a token with exp 100 is still accepted over HTTP (`100 < floor(100.5)` is false), so a
  // connection opened with it is not closed yet; one second later it is.
  const real = Date.now;
  try {
    Date.now = () => 100_500;
    assert.equal(webSocketAuthExpired({ exp: 100 }), false);
    Date.now = () => 101_000;
    assert.equal(webSocketAuthExpired({ exp: 100 }), true);
  } finally {
    Date.now = real;
  }
});
