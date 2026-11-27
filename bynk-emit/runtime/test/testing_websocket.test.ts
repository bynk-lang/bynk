// #3901: the in-process Workers WebSocket surface the `system` tier drives an upgrade through.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import {
  TestWebSocketPair,
  installTestWebSocketRuntime,
  makeHibernatableTestState,
  takeWebSocketFaults,
  webSocketQuiet,
  type TestWebSocket,
  type WebSocketHost,
} from "../src/testing/websocket.ts";
import { makeIntegrationDoNamespace } from "../src/testing/bindings.ts";
import { acceptHibernatableConnection, newWebSocketPair, webSocketUpgradeResponse } from "../src/connection.ts";

function host() {
  const seen: { message: unknown[]; close: unknown[][] } = { message: [], close: [] };
  const h: WebSocketHost = {
    webSocketMessage: (ws, m) => void seen.message.push([ws.deserializeAttachment(), m]),
    webSocketClose: (_ws, code, reason, clean) => void seen.close.push([code, reason, clean]),
  };
  return { h, seen };
}

function hosted(h: WebSocketHost, tags: string[] = ["c1"]) {
  const pair = new TestWebSocketPair();
  const state = makeHibernatableTestState("room", () => h);
  state.acceptWebSocket(pair[1], tags);
  pair[1].serializeAttachment({ connId: "c1", n: 1 });
  return { client: pair[0], server: pair[1], state };
}

test("a frame sent on one end reaches the other in order, asynchronously, binary as an ArrayBuffer", async () => {
  const pair = new TestWebSocketPair();
  pair[1].accept();
  const got: unknown[] = [];
  pair[1].addEventListener("message", (e) => got.push((e as { data: unknown }).data));
  pair[0].send("a");
  pair[0].send(new Uint8Array([1, 2]));
  pair[0].send("b");
  assert.deepEqual(got, []);
  await webSocketQuiet();
  assert.equal(got[0], "a");
  const binary = got[1] as object;
  assert.ok(binary instanceof ArrayBuffer && new Uint8Array(binary).join() === "1,2");
  assert.equal(got[2], "b");
});

test("an accepted socket's frames reach the object's webSocketMessage with its attachment, not its listeners", async () => {
  const { h, seen } = host();
  const { client, server } = hosted(h);
  let evented = false;
  server.addEventListener("message", () => (evented = true));
  client.send("hi");
  await webSocketQuiet();
  assert.deepEqual(seen.message, [[{ connId: "c1", n: 1 }, "hi"]]);
  assert.equal(evented, false);
});

test("a client close reaches webSocketClose and ends both ends; the client is told with the code", async () => {
  const { h, seen } = host();
  const { client, server, state } = hosted(h);
  const told: unknown[] = [];
  client.addEventListener("close", (e) => told.push(e));
  client.close(1000, "bye");
  assert.equal(client.readyState, 2);
  await webSocketQuiet();
  assert.deepEqual(seen.close, [[1000, "bye", true]]);
  assert.equal(client.readyState, 3);
  assert.equal(server.readyState, 3);
  assert.equal(told.length, 1);
  assert.deepEqual(client.closeEvent, { code: 1000, reason: "bye", wasClean: true });
  assert.deepEqual(state.getWebSockets("c1"), []);
});

test("a server close tells the client; a close on a closed socket is ignored", async () => {
  const { h, seen } = host();
  const { client, server } = hosted(h);
  server.close(1008, "authentication expired");
  server.close(1000, "again");
  await webSocketQuiet();
  assert.deepEqual(client.closeEvent, { code: 1008, reason: "authentication expired", wasClean: true });
  assert.deepEqual(seen.close, []);
  client.close(1000);
  await webSocketQuiet();
  assert.deepEqual(client.closeEvent?.code, 1008);
});

test("the close codes a page or the platform may not send are refused", () => {
  const { client, server } = hosted(host().h);
  assert.throws(() => client.close(1008, "no"), /InvalidAccessError/);
  assert.throws(() => server.close(1006), /reserved/);
  assert.throws(() => server.close(1000, "x".repeat(124)), /123 bytes/);
  assert.equal(client.readyState, 1);
});

test("a dropped network tells both ends 1006, unclean", async () => {
  const { h, seen } = host();
  const { client } = hosted(h);
  client.terminate();
  await webSocketQuiet();
  assert.deepEqual(seen.close, [[1006, "", false]]);
  assert.deepEqual(client.closeEvent, { code: 1006, reason: "", wasClean: false });
});

test("getWebSockets finds an accepted socket by tag and forgets a closed one", async () => {
  const { h } = host();
  const state = makeHibernatableTestState("room", () => h);
  const a = new TestWebSocketPair();
  const b = new TestWebSocketPair();
  state.acceptWebSocket(a[1], ["a"]);
  state.acceptWebSocket(b[1], ["b"]);
  assert.deepEqual(state.getWebSockets("a"), [a[1]]);
  assert.equal(state.getWebSockets().length, 2);
  a[0].close(1000);
  await webSocketQuiet();
  assert.deepEqual(state.getWebSockets("a"), []);
  assert.deepEqual(state.getWebSockets("b"), [b[1]]);
});

test("a handler that throws is recorded for the test, not lost", async () => {
  takeWebSocketFaults();
  const { client } = hosted({
    webSocketMessage: () => {
      throw new Error("boom");
    },
  });
  client.send("x");
  await webSocketQuiet();
  const faults = takeWebSocketFaults();
  assert.equal(faults.length, 1);
  assert.match(String((faults[0] as Error).message), /boom/);
  assert.deepEqual(takeWebSocketFaults(), []);
});

test("an attachment is a structured clone: later writes do not change what was read", () => {
  const pair = new TestWebSocketPair();
  const value = { n: 1 };
  pair[1].serializeAttachment(value);
  value.n = 2;
  assert.deepEqual(pair[1].deserializeAttachment(), { n: 1 });
  assert.equal(new TestWebSocketPair()[1].deserializeAttachment(), null);
});

test("installed, the upgrade code the Worker runs builds a 101 carrying the client end; uninstalled, it is as before", () => {
  const before = globalThis.Response;
  const restore = installTestWebSocketRuntime();
  try {
    const { client } = newWebSocketPair();
    const response = webSocketUpgradeResponse(client, "bynk.bearer");
    assert.equal(response.status, 101);
    assert.equal(response.ok, false);
    assert.equal((response as unknown as { webSocket: unknown }).webSocket, client);
    assert.equal(response.headers.get("sec-websocket-protocol"), "bynk.bearer");
    assert.ok(new Response("x", { status: 200 }) instanceof Response);
    assert.ok(before === globalThis.Response ? false : new before("x") instanceof Response);
    // The upgrade request a client sends is an ordinary `Request`.
    const request = new Request("https://w/rooms/r1", { headers: { Upgrade: "websocket", "Sec-WebSocket-Protocol": "bynk.bearer, t" } });
    assert.equal(request.headers.get("upgrade"), "websocket");
  } finally {
    restore();
  }
  assert.equal(globalThis.Response, before);
  assert.throws(() => new Response(null, { status: 101 }), RangeError);
  assert.equal((globalThis as { WebSocketPair?: unknown }).WebSocketPair, undefined);
});

test("a Durable Object namespace hosts the upgrade: the stub's fetch returns the 101, frames reach the instance", async () => {
  const restore = installTestWebSocketRuntime();
  try {
    const log: string[] = [];
    class Room {
      state: ReturnType<typeof makeHibernatableTestState>;
      constructor(state: ReturnType<typeof makeHibernatableTestState>) {
        this.state = state;
      }
      async fetch(_request: Request): Promise<Response> {
        const { client, server } = newWebSocketPair();
        acceptHibernatableConnection(this.state as never, server, { who: "ann" });
        return webSocketUpgradeResponse(client, "bynk.bearer");
      }
      async webSocketMessage(ws: TestWebSocket, message: string): Promise<void> {
        log.push(`${(ws.deserializeAttachment() as { who: string }).who}:${message}`);
        ws.send(`echo ${message}`);
      }
      async webSocketClose(_ws: TestWebSocket, code: number): Promise<void> {
        log.push(`closed ${code}`);
      }
    }
    const ns = makeIntegrationDoNamespace(Room as never);
    const response = await ns.get(ns.idFromName("r1")).fetch("https://_bynk/_bynk/ws/open/Chat", {
      headers: { Upgrade: "websocket" },
    });
    assert.equal(response.status, 101);
    const client = (response as unknown as { webSocket: TestWebSocket }).webSocket;
    const heard: unknown[] = [];
    client.addEventListener("message", (e) => heard.push((e as { data: unknown }).data));
    client.send("hello");
    await webSocketQuiet();
    assert.deepEqual(heard, ["echo hello"]);
    client.close(1000);
    await webSocketQuiet();
    assert.deepEqual(log, ["ann:hello", "closed 1000"]);
    assert.deepEqual(takeWebSocketFaults(), []);
  } finally {
    restore();
  }
});

// #3901: the doubles are for a test build only. The production runtime modules never import them, so a
// deployed Worker cannot reach `installTestWebSocketRuntime` or anything that patches a global.
test("no production runtime module imports the test WebSocket doubles", () => {
  const src = join(import.meta.dirname, "..", "src");
  const offenders: string[] = [];
  const walk = (dir: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) {
        if (entry.name !== "testing") walk(path);
      } else if (entry.name.endsWith(".ts") && /testing\/websocket|installTestWebSocketRuntime/.test(readFileSync(path, "utf8"))) {
        offenders.push(path);
      }
    }
  };
  walk(src);
  assert.deepEqual(offenders, []);
});

test("two installs share the patch: it is put back only when the last of them lets go", () => {
  const before = globalThis.Response;
  const first = installTestWebSocketRuntime();
  const second = installTestWebSocketRuntime();
  assert.notEqual(globalThis.Response, before);
  first();
  first();
  assert.notEqual(globalThis.Response, before);
  assert.equal(typeof (globalThis as { WebSocketPair?: unknown }).WebSocketPair, "function");
  second();
  assert.equal(globalThis.Response, before);
  assert.equal((globalThis as { WebSocketPair?: unknown }).WebSocketPair, undefined);
});

test("webSocketQuiet that runs out of turns is an error naming the cause, not a return that reads as quiet", async () => {
  let rounds = 0;
  const pair = new TestWebSocketPair();
  const state = makeHibernatableTestState("room", () => ({
    webSocketMessage: async (ws: TestWebSocket): Promise<void> => {
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
      rounds += 1;
      if (rounds < 20) ws.send("again");
    },
  }));
  state.acceptWebSocket(pair[1]);
  pair[0].addEventListener("message", () => pair[0].send("again"));
  pair[0].send("go");
  await assert.rejects(() => webSocketQuiet(3), /\d+ WebSocket deliveries still in flight after 3 turns/);
  // Left alone it settles, and a patient wait says so.
  await webSocketQuiet();
  assert.equal(takeWebSocketFaults().length, 0);
});

test("a listener that throws is kept with the faults, not thrown into the event loop", async () => {
  takeWebSocketFaults();
  const pair = new TestWebSocketPair();
  pair[1].accept();
  pair[1].addEventListener("message", () => {
    throw new Error("listener boom");
  });
  pair[0].send("x");
  await webSocketQuiet();
  const faults = takeWebSocketFaults();
  assert.equal(faults.length, 1);
  assert.match(String((faults[0] as Error).message), /listener boom/);
});
