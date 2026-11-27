import { test } from "node:test";
import assert from "node:assert/strict";
import type { ClientOptions } from "../src/client/http_client.ts";
import { __webSocket, type __WsSpec } from "../src/client/http_client_ws.ts";
import { Err, Ok } from "../src/result.ts";

// A scriptable stand-in for the platform `WebSocket`.
class FakeSocket extends EventTarget {
  static made: FakeSocket[] = [];
  binaryType = "blob";
  sent: unknown[] = [];
  closed: { code?: number; reason?: string } | undefined;
  readonly url: string;
  readonly protocols: string | string[] | undefined;
  constructor(url: string, protocols?: string | string[]) {
    super();
    this.url = url;
    this.protocols = protocols;
    FakeSocket.made.push(this);
  }
  send(data: unknown): void {
    this.sent.push(data);
  }
  close(code?: number, reason?: string): void {
    this.closed = { code, reason };
    queueMicrotask(() => this.finish(code ?? 1005, reason ?? "", true));
  }
  open(): void {
    this.dispatchEvent(new Event("open"));
  }
  message(data: unknown): void {
    this.dispatchEvent(Object.assign(new Event("message"), { data }));
  }
  finish(code: number, reason: string, wasClean: boolean): void {
    this.dispatchEvent(Object.assign(new Event("close"), { code, reason, wasClean }));
  }
}

const options = (): ClientOptions => ({ baseUrl: "https://api.example.com/", webSocket: FakeSocket as unknown as typeof WebSocket });
const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 0));

// A frame is `{ n: number }`; negative numbers are refused, as a refinement would.
const spec = (credential: __WsSpec<{ n: number }, { n: number }>["credential"] = "bearer"): __WsSpec<{ n: number }, { n: number }> => ({
  credential,
  binaryIn: false,
  binaryOut: false,
  encode: (f) => (f.n < 0 ? Err({ kind: "StructuralMismatch", path: "$.n", expected: "non-negative", actual: String(f.n) }) : Ok(f as never)),
  decode: (j) => (typeof (j as { n?: unknown }).n === "number" ? Ok(j as { n: number }) : Err({ kind: "StructuralMismatch", path: "$", expected: "{ n }", actual: "?" })),
});

const build = () => Ok({ params: { room: "r 1" } });

function connect(s = spec(), credentials: unknown = { bearer: "tok" }, extra?: object) {
  FakeSocket.made = [];
  const c = __webSocket(options(), s, build, credentials, extra) as ReturnType<typeof __webSocket> & any;
  return c;
}

test("the upgrade goes to a ws URL with the route's parameters, offering the bearer pair", async () => {
  const c = connect();
  await tick();
  const [socket] = FakeSocket.made;
  assert.equal(socket.url, "wss://api.example.com/?room=r+1");
  assert.deepEqual(socket.protocols, ["bynk.bearer", "tok"]);
  assert.equal(socket.binaryType, "arraybuffer");
  assert.equal(c.state, "connecting");
  socket.open();
  await c.opened;
  assert.equal(c.state, "open");
});

test("an oidc token is offered with its own marker, a cookie service offers none, and a token may be a function", async () => {
  connect(spec("oidc"), { oidc: async () => "jwt" });
  await tick();
  assert.deepEqual(FakeSocket.made[0].protocols, ["bynk.oidc", "jwt"]);
  connect(spec("cookie"), undefined);
  await tick();
  assert.equal(FakeSocket.made[0].protocols, undefined);
  connect(spec("bearer?"), {});
  await tick();
  assert.equal(FakeSocket.made[0].protocols, undefined);
});

test("a frame waits for the open, an invalid one is refused locally, and a closed connection says so", async () => {
  const c = connect();
  await tick();
  assert.equal(c.send({ n: 1 }), null);
  assert.equal(FakeSocket.made[0].sent.length, 0);
  const bad = c.send({ n: -1 });
  assert.equal(bad.kind, "Invalid");
  FakeSocket.made[0].open();
  assert.deepEqual(FakeSocket.made[0].sent, ['{"n":1}']);
  assert.equal(c.send({ n: 2 }), null);
  assert.deepEqual(FakeSocket.made[0].sent, ['{"n":1}', '{"n":2}']);
  c.close();
  await tick();
  assert.equal(c.state, "closed");
  assert.equal(c.send({ n: 3 }).kind, "Transport");
});

test("frames arrive decoded, to listeners and to the iterator", async () => {
  const c = connect();
  await tick();
  const seen: number[] = [];
  c.onFrame((f: { n: number }) => seen.push(f.n));
  const it = c.frames[Symbol.asyncIterator]();
  FakeSocket.made[0].open();
  FakeSocket.made[0].message('{"n":7}');
  FakeSocket.made[0].message('{"n":8}');
  assert.deepEqual(seen, [7, 8]);
  assert.deepEqual(await it.next(), { done: false, value: { n: 7 } });
  assert.deepEqual(await it.next(), { done: false, value: { n: 8 } });
  const last = it.next();
  c.close();
  assert.deepEqual(await last, { done: true, value: undefined });
});

test("a frame that does not decode closes the connection with 4008 and does not reconnect", async () => {
  const closes: unknown[] = [];
  const c = connect(spec(), { bearer: "t" }, { reconnect: { credentials: () => ({ bearer: "t2" }), backoff: () => 0 } });
  await tick();
  c.onClose((x: unknown) => closes.push(x));
  FakeSocket.made[0].open();
  FakeSocket.made[0].message('{"n":"x"}');
  await tick();
  assert.deepEqual(FakeSocket.made[0].closed, { code: 4008, reason: "invalid frame" });
  assert.deepEqual(closes, [{ code: 4008, reason: "invalid frame", clean: true, willReconnect: false }]);
  assert.equal(FakeSocket.made.length, 1);
  assert.equal(c.state, "closed");
});

test("a text frame where binary is expected is refused the same way", async () => {
  const s = { ...spec(), binaryOut: true, decode: (j: unknown) => Ok(j as never) };
  const c = connect(s);
  await tick();
  FakeSocket.made[0].open();
  FakeSocket.made[0].message("text");
  await tick();
  assert.equal(FakeSocket.made[0].closed?.code, 4008);
  assert.equal(c.state, "closed");
});

test("an expired credential reconnects with a fresh one, and the frames carry on", async () => {
  const reconnects: number[] = [];
  const closes: { code: number; willReconnect: boolean }[] = [];
  let issued = 0;
  const c = connect(spec(), { bearer: "old" }, {
    reconnect: { credentials: () => ({ bearer: `new${++issued}` }), backoff: () => 0 },
  });
  await tick();
  c.onClose((x: { code: number; willReconnect: boolean }) => closes.push(x));
  c.onReconnect((n: number) => reconnects.push(n));
  const seen: number[] = [];
  c.onFrame((f: { n: number }) => seen.push(f.n));
  FakeSocket.made[0].open();
  FakeSocket.made[0].message('{"n":1}');
  FakeSocket.made[0].finish(1008, "authentication expired", true);
  await tick();
  await tick();
  assert.equal(FakeSocket.made.length, 2);
  assert.deepEqual(FakeSocket.made[1].protocols, ["bynk.bearer", "new1"]);
  assert.equal(c.state, "reconnecting");
  assert.equal(c.send({ n: 5 }).kind, "Transport");
  FakeSocket.made[1].open();
  FakeSocket.made[1].message('{"n":2}');
  assert.deepEqual(seen, [1, 2]);
  assert.deepEqual(reconnects, [1]);
  assert.deepEqual(closes, [{ code: 1008, reason: "authentication expired", clean: true, willReconnect: true }]);
  assert.equal(c.state, "open");
});

test("an abnormal close reconnects up to maxAttempts, then stays closed; another 1008 does not reconnect", async () => {
  const c = connect(spec(), { bearer: "t" }, { reconnect: { credentials: () => ({ bearer: "t" }), backoff: () => 0, maxAttempts: 2 } });
  await tick();
  for (let i = 0; i < 3; i++) {
    FakeSocket.made[i].finish(1006, "", false);
    await tick();
    await tick();
  }
  assert.equal(FakeSocket.made.length, 3);
  assert.equal(c.state, "closed");
  await assert.rejects(c.opened);
  const d = connect(spec(), { bearer: "t" }, { reconnect: { credentials: () => ({ bearer: "t" }), backoff: () => 0 } });
  await tick();
  FakeSocket.made[0].open();
  FakeSocket.made[0].finish(1008, "forbidden", true);
  await tick();
  assert.equal(FakeSocket.made.length, 1);
  assert.equal(d.state, "closed");
});

test("without reconnect, a close ends the connection and the iterator", async () => {
  const c = connect();
  await tick();
  const it = c.frames[Symbol.asyncIterator]();
  FakeSocket.made[0].open();
  FakeSocket.made[0].finish(1006, "", false);
  assert.deepEqual(await it.next(), { done: true, value: undefined });
  assert.equal(c.state, "closed");
});

test("a bad route parameter, a missing credential, and no WebSocket are failures, never throws", async () => {
  const refused = __webSocket(options(), spec(), () => Err({ kind: "StructuralMismatch", path: "room", expected: "x", actual: "y" }), {}, undefined) as { kind: string };
  assert.equal(refused.kind, "Invalid");
  const had = (globalThis as { WebSocket?: unknown }).WebSocket;
  (globalThis as { WebSocket?: unknown }).WebSocket = undefined;
  try {
    const none = __webSocket({ baseUrl: "https://x" }, spec(), build, { bearer: "t" }, undefined) as { state: string };
    await tick();
    assert.equal(none.state, "closed");
  } finally {
    (globalThis as { WebSocket?: unknown }).WebSocket = had;
  }
  const relative = __webSocket({ webSocket: FakeSocket as never }, spec(), build, { bearer: "t" }, undefined) as { kind: string };
  assert.equal(relative.kind, "Transport");
  FakeSocket.made = [];
  const missing = __webSocket(options(), spec(), build, {}, undefined) as { state: string };
  await tick();
  assert.equal(missing.state, "closed");
  assert.equal(FakeSocket.made.length, 0);
});

test("a refused upgrade that fires only `error`, as Node does, is the end of that attempt, once", async () => {
  const closes: unknown[] = [];
  const c = connect();
  await tick();
  c.onClose((x: unknown) => closes.push(x));
  FakeSocket.made[0].dispatchEvent(new Event("error"));
  FakeSocket.made[0].finish(1006, "", false);
  await assert.rejects(c.opened);
  assert.equal(c.state, "closed");
  assert.deepEqual(closes, [{ code: 1006, reason: "", clean: false, willReconnect: false }]);
});

test("binary frames cross as Uint8Array both ways", async () => {
  const s = {
    credential: "none",
    binaryIn: true,
    binaryOut: true,
    encode: (f: Uint8Array) => Ok(f),
    decode: (j: unknown) => Ok(j as Uint8Array),
  } as unknown as __WsSpec<Uint8Array, Uint8Array>;
  const c = connect(s as never, undefined);
  await tick();
  FakeSocket.made[0].open();
  const sent = new Uint8Array([1, 2, 3]);
  assert.equal(c.send(sent), null);
  assert.equal(FakeSocket.made[0].sent[0], sent);
  const seen: Uint8Array[] = [];
  c.onFrame((f: Uint8Array) => seen.push(f));
  FakeSocket.made[0].message(new Uint8Array([9, 8]).buffer);
  assert.deepEqual([...seen[0]], [9, 8]);
});

test("a frame queued before a close is never resent on a reconnect, and credentials are asked for at every attempt", async () => {
  let asked = 0;
  const c = connect(spec(), { bearer: "old" }, {
    reconnect: { credentials: () => ({ bearer: `fresh${++asked}` }), backoff: () => 0, maxAttempts: 3 },
  });
  await tick();
  // Queued while the first attempt is still opening, then the attempt fails.
  assert.equal(c.send({ n: 1 }), null);
  FakeSocket.made[0].finish(1006, "", false);
  await tick();
  await tick();
  assert.equal(FakeSocket.made.length, 2);
  assert.equal(c.send({ n: 2 }).kind, "Transport");
  FakeSocket.made[1].finish(1006, "", false);
  await tick();
  await tick();
  assert.equal(FakeSocket.made.length, 3);
  assert.deepEqual(FakeSocket.made[2].protocols, ["bynk.bearer", "fresh2"]);
  FakeSocket.made[2].open();
  // Nothing written before the closes reaches the connection that finally opened.
  assert.deepEqual(FakeSocket.made[2].sent, []);
  assert.equal(c.send({ n: 3 }), null);
  assert.deepEqual(FakeSocket.made[2].sent, ['{"n":3}']);
  // One credential call per attempt, not one per connection.
  assert.equal(asked, 2);
  assert.deepEqual(FakeSocket.made.slice(1).map((s) => s.protocols), [["bynk.bearer", "fresh1"], ["bynk.bearer", "fresh2"]]);
});
