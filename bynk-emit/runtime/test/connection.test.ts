// #3051: the case-side `TestConnection` — what `svc.open(tc, …)` remembers, and
// the one-owner rule that stops a handle being opened and handed out too.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  BEARER_SUBPROTOCOL,
  OIDC_SUBPROTOCOL,
  TestConnection,
  WorkersConnection,
  registerFrameEncoder,
  closeArguments,
  webSocketAdmit,
  webSocketAuthExpired,
  webSocketClosing,
  webSocketErrored,
  webSocketBearerOffer,
  webSocketBearerProtocol,
  webSocketBearerRefusal,
  webSocketOriginAllowed,
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

test("an Oidc upgrade offers its own marker, and the Bearer marker is not it (#3904)", async () => {
  assert.equal(OIDC_SUBPROTOCOL, "bynk.oidc");
  assert.deepEqual(webSocketBearerOffer("bynk.oidc, tok.en.sig", OIDC_SUBPROTOCOL), { tag: "Token", token: "tok.en.sig" });
  assert.deepEqual(webSocketBearerOffer("bynk.bearer, tok.en.sig", OIDC_SUBPROTOCOL), { tag: "Legacy" });
  assert.deepEqual(webSocketBearerOffer("bynk.oidc", OIDC_SUBPROTOCOL), { tag: "Absent" });
  assert.equal(webSocketBearerProtocol("bynk.oidc, tok", OIDC_SUBPROTOCOL), "bynk.oidc");
  assert.equal(webSocketBearerProtocol("bynk.bearer, tok", OIDC_SUBPROTOCOL), undefined);
  const res = webSocketBearerRefusal(OIDC_SUBPROTOCOL);
  assert.equal(res.status, 400);
  assert.equal(await res.text(), 'Offer the subprotocols ["bynk.oidc", <token>]');
});

test("a cookie upgrade's Origin must be exactly a listed one (#3904)", () => {
  const listed = ["https://app.example.com", "https://admin.example.com:8443"];
  assert.equal(webSocketOriginAllowed("https://app.example.com", listed), true);
  assert.equal(webSocketOriginAllowed("https://admin.example.com:8443", listed), true);
  for (const origin of [
    null,
    "",
    "null",
    "*",
    "https://evil.example",
    "https://app.example.com.evil.example",
    "http://app.example.com",
    "https://app.example.com:8443",
    "https://APP.example.com",
    "https://app.example.com/",
  ]) {
    assert.equal(webSocketOriginAllowed(origin, listed), false, String(origin));
  }
  // Nothing listed, or a wildcard listed, trusts nothing.
  assert.equal(webSocketOriginAllowed("https://app.example.com", []), false);
  assert.equal(webSocketOriginAllowed("*", ["*"]), false);
  assert.equal(webSocketOriginAllowed("null", ["null"]), false);
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

test("webSocketClosing echoes the code, then runs the handler with the peer's details and marks the socket", async () => {
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

test("the close is sent even when the handler faults, and the fault still propagates", async () => {
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

test("the close goes out before on close runs, so a release that closes the socket cannot replace its code", async () => {
  const s = socket();
  const order: string[] = [];
  const close = s.ws.close;
  s.ws.close = (...a: unknown[]) => {
    order.push("close");
    close(...a);
  };
  await webSocketClosing(
    s.ws,
    { connId: "c" },
    { code: 4001, reason: "bye", clean: true },
    async () => void order.push("on close"),
  );
  assert.deepEqual(order, ["close", "on close"]);
  assert.deepEqual(s.closes, [[4001, "bye"]], "and it is not sent again afterwards");
});

// -- #3905: frame limits and binary frames ----------------------------------------------------------

function fakeSocket(attachment: unknown = null) {
  const sent: (string | Uint8Array)[] = [];
  const closed: { code?: number; reason?: string }[] = [];
  return {
    sent,
    closed,
    send(data: string | Uint8Array) {
      sent.push(data);
    },
    close(code?: number, reason?: string) {
      closed.push({ code, reason });
    },
    deserializeAttachment: () => attachment,
  };
}

test("an inbound frame over maxFrame closes with 1009 before anything is parsed (#3905)", () => {
  const log = console.error;
  const logged: string[] = [];
  console.error = (m: string) => logged.push(m);
  try {
    for (const binary of [false, true]) {
      const ws = fakeSocket();
      const frame: string | ArrayBuffer = binary ? new ArrayBuffer(9) : "x".repeat(9);
      assert.equal(webSocketAdmit(ws, frame, 8, binary), false);
      assert.deepEqual(ws.closed, [{ code: 1009, reason: "message too large" }]);
    }
  } finally {
    console.error = log;
  }
  // The log names the size and nothing of the content.
  assert.ok(logged.every((m) => /9 bytes/.test(m) && !m.includes("xxx")), logged.join("|"));
});

test("maxFrame counts bytes of UTF-8, not characters (#3905)", () => {
  // "é" is two bytes: four of them are 8 bytes, which fits 8, and five are 10, which does not.
  const ok = fakeSocket();
  assert.equal(webSocketAdmit(ok, "éééé", 8, false), true);
  assert.deepEqual(ok.closed, []);
  const big = fakeSocket();
  assert.equal(webSocketAdmit(big, "ééééé", 8, false), false);
  assert.equal(big.closed[0]!.code, 1009);
  // Three-byte characters and a surrogate pair (four bytes in two UTF-16 units).
  assert.equal(webSocketAdmit(fakeSocket(), "€€", 6, false), true);
  assert.equal(webSocketAdmit(fakeSocket(), "€€", 5, false), false);
  assert.equal(webSocketAdmit(fakeSocket(), "😀", 4, false), true);
  assert.equal(webSocketAdmit(fakeSocket(), "😀", 3, false), false);
  // At the limit exactly is admitted.
  assert.equal(webSocketAdmit(fakeSocket(), "x".repeat(8), 8, false), true);
});

test("a frame of the wrong kind closes with 1003 (#3905)", () => {
  const text = fakeSocket();
  assert.equal(webSocketAdmit(text, "hi", 1024, true), false);
  assert.deepEqual(text.closed, [{ code: 1003, reason: "text frame not accepted" }]);
  const bin = fakeSocket();
  assert.equal(webSocketAdmit(bin, new ArrayBuffer(2), 1024, false), false);
  assert.deepEqual(bin.closed, [{ code: 1003, reason: "binary frame not accepted" }]);
  // The right kinds go on.
  assert.equal(webSocketAdmit(fakeSocket(), "hi", 1024, false), true);
  assert.equal(webSocketAdmit(fakeSocket(), new ArrayBuffer(2), 1024, true), true);
});

test("an oversize check comes before the kind check (#3905)", () => {
  const ws = fakeSocket();
  assert.equal(webSocketAdmit(ws, "x".repeat(20), 8, true), false);
  assert.equal(ws.closed[0]!.code, 1009);
});

test("a Bytes frame is sent as a binary message and any other as JSON text (#3905)", async () => {
  const ws = fakeSocket({ connId: "c", max: 64, bin: true });
  const conn = new WorkersConnection<unknown>(ws, "c");
  const bytes = new Uint8Array([1, 2, 3]);
  await conn.send(bytes);
  await conn.send({ text: "hi" });
  assert.equal(ws.sent[0], bytes);
  assert.equal(ws.sent[1], '{"text":"hi"}');
  // A service that is not `out: Bytes` records no `bin`, and sends JSON, whatever the frame's representation.
  const json = fakeSocket({ connId: "c", max: 64 });
  await new WorkersConnection<unknown>(json, "c").send({ text: "hi" });
  assert.equal(json.sent[0], '{"text":"hi"}');
  const notBin = fakeSocket({ connId: "c", max: 64 });
  await new WorkersConnection<unknown>(notBin, "c").send(new Uint8Array([1]));
  assert.equal(typeof notBin.sent[0], "string");
});

test("a frame the server sends over the attachment's maxFrame faults the send (#3905)", async () => {
  const ws = fakeSocket({ connId: "c", max: 8, bin: true });
  const conn = new WorkersConnection<unknown>(ws, "c");
  await conn.send(new Uint8Array(8));
  await assert.rejects(conn.send(new Uint8Array(9)), /9 bytes is over the service's maxFrame of 8/);
  await assert.rejects(conn.send({ text: "xxxxxxxx" }), /over the service's maxFrame of 8/);
  assert.equal(ws.sent.length, 1);
  // A socket with no ceiling recorded (accepted before it existed) sends as before.
  const old = fakeSocket({ connId: "c" });
  await new WorkersConnection<unknown>(old, "c").send(new Uint8Array(1000));
  assert.equal(old.sent.length, 1);
});

test("a TestConnection opened by a service holds that service's maxFrame (#3905)", async () => {
  const tc = new TestConnection<Uint8Array>();
  await tc.send(new Uint8Array(1000));
  const bounded = new TestConnection<Uint8Array>();
  bounded.remember([], {}, 8);
  await bounded.send(new Uint8Array(8));
  await assert.rejects(bounded.send(new Uint8Array(9)), /over the service's maxFrame of 8/);
  assert.equal(bounded.sent.length, 1);
});

test("a JSON frame is written by the codec registered for the service its socket names (#4549)", async () => {
  // The runtime's own shape of an `Option` is `{ tag }`; the codec's wire form is `{ kind }`.
  registerFrameEncoder<{ note: { tag: "None" } | { tag: "Some"; value: string } }>("codec-test", (f) => ({
    note: f.note.tag === "Some" ? { kind: "Some", value: f.note.value } : { kind: "None" },
  }));
  const ws = fakeSocket({ connId: "c", max: 1000, svc: "codec-test" });
  const conn = new WorkersConnection<{ note: { tag: "None" } | { tag: "Some"; value: string } }>(ws, "c");
  await conn.send({ note: { tag: "Some", value: "x" } });
  await conn.send({ note: { tag: "None" } });
  assert.deepEqual(ws.sent, ['{"note":{"kind":"Some","value":"x"}}', '{"note":{"kind":"None"}}']);
  // The size ceiling is judged on what goes out, not on the runtime shape.
  const small = fakeSocket({ connId: "c", max: 20, svc: "codec-test" });
  await assert.rejects(new WorkersConnection<never>(small, "c").send({ note: { tag: "Some", value: "xxxxxxxxxxxx" } } as never), /over the service's maxFrame of 20/);
  // A socket that names no service, or one nothing registered, is sent as it always was.
  const old = fakeSocket({ connId: "c" });
  await new WorkersConnection<unknown>(old, "c").send({ tag: "None" });
  assert.equal(old.sent[0], '{"tag":"None"}');
  const unknown = fakeSocket({ connId: "c", svc: "not-registered" });
  await new WorkersConnection<unknown>(unknown, "c").send({ a: 1 });
  assert.equal(unknown.sent[0], '{"a":1}');
  // A binary service keeps its frames binary: it has no `svc`, so no codec.
  const bin = fakeSocket({ connId: "c", bin: true, svc: "codec-test" });
  await new WorkersConnection<unknown>(bin, "c").send(new Uint8Array([1, 2]));
  assert.ok(bin.sent[0] instanceof Uint8Array);
});

test("a TestConnection runs the service's codec, keeps the frame as written, and sizes the wire form (#4549)", async () => {
  const tc = new TestConnection<{ n: bigint }>();
  tc.remember([], {}, 12, (f) => ({ n: f.n.toString() }));
  await tc.send({ n: 12n });
  assert.deepEqual(tc.sent, [{ n: 12n }]);
  await assert.rejects(tc.send({ n: 123456789n }), /over the service's maxFrame of 12/);
  // With no codec handed over, as before.
  const plain = new TestConnection<{ n: number }>();
  await plain.send({ n: 1 });
  assert.deepEqual(plain.sent, [{ n: 1 }]);
});
