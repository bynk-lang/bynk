// #3051: the case-side `TestConnection` — what `svc.open(tc, …)` remembers, and
// the one-owner rule that stops a handle being opened and handed out too.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  BEARER_SUBPROTOCOL,
  TestConnection,
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
