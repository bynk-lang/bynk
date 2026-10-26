// v0.102 (real-time track slice 2): the runtime contract for a held
// `Connection[F]`. A `Connection<F>` is a typed handle to a long-lived channel;
// `send` writes a server→client frame, `close` ends the connection. The
// concrete implementations — a capture-and-inspect `TestConnection` for the
// bundle/test target and the hibernatable-WebSocket binding for Workers — arrive
// with the `from WebSocket` protocol (slice 3); the language emits against this
// interface, and the linearity discipline (§2.9) governs ownership at the type
// level.
import { Some, None, type Option } from "./result.ts";
import type { DurableObjectState } from "./storage.ts";

export interface Connection<F> {
  send(frame: F): Promise<void>;
  close(): Promise<void>;
}

// v0.103: the bundle/test realisation of a `Connection[F]` — a capture-and-inspect
// channel. The runtime-managed lifecycle in production becomes an inspectable
// record in tests (design notes §20): `sent` holds every frame `send`-given, and
// `closed` records disposal. A test drives an `on open` handler with a
// `TestConnection` and asserts what the agent sent.
export class TestConnection<F> implements Connection<F> {
  readonly sent: F[] = [];
  closed = false;

  async send(frame: F): Promise<void> {
    if (this.closed) throw new Error("send on a closed TestConnection");
    this.sent.push(frame);
  }

  async close(): Promise<void> {
    this.closed = true;
  }
}

// v0.104/v0.105 (real-time track slice 3b): the Cloudflare Workers realisation of
// a `Connection[F]`, wrapping a server-side `WebSocket` accepted in a Durable
// Object. A frame is sent as JSON; `close` ends the socket. The `connId` is the
// hibernation tag the socket was accepted under (slice 3b-ii): a stored connection
// persists this id, and `resolveConnection` re-presents the live socket by it
// after the DO wakes — so the held value survives eviction (§2.9.6).
export class WorkersConnection<F> implements Connection<F> {
  // Explicit fields + assignment, not constructor parameter properties — Node's
  // `--experimental-strip-types` (the `--inspect` debug path runs the emitted `.ts`
  // directly) rejects parameter properties, which are not erasable.
  private readonly ws: { send(data: string): void; close(): void };
  readonly connId: string;

  constructor(ws: { send(data: string): void; close(): void }, connId: string) {
    this.ws = ws;
    this.connId = connId;
  }

  async send(frame: F): Promise<void> {
    this.ws.send(JSON.stringify(frame));
  }

  async close(): Promise<void> {
    this.ws.close();
  }
}

// v0.105 (slice 3b-ii): the connection id a held `Connection` persists. The stored
// value is this string, not the live socket (which cannot be serialised); the
// socket is re-resolved per access via `resolveConnection`.
export function connIdOf(conn: Connection<unknown>): string {
  return (conn as WorkersConnection<unknown>).connId;
}

// v0.104 (real-time track slice 3b): the minimal structural surface of a
// Cloudflare server-side `WebSocket`, so emitted Worker code type-checks under
// `tsc --strict` without depending on `@cloudflare/workers-types`. The real
// runtime object is richer but compatible.
export interface WorkersWebSocket {
  accept(): void;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  // v0.105 (slice 3b-ii): hibernation attachment — small per-socket data the
  // platform preserves across hibernation (the connId, so a waking dispatch can
  // recover it without loading durable state).
  serializeAttachment(value: unknown): void;
  deserializeAttachment(): unknown;
}

// v0.105 (slice 3b-ii): the Durable Object's hibernatable-WebSocket surface.
// `acceptWebSocket(ws, tags)` accepts a server socket so it survives the DO's
// hibernation (unlike `ws.accept()`); `getWebSockets(tag)` re-presents the
// accepted sockets (by tag) when the DO wakes. Narrowly typed and reached by a
// cast so the shared `DurableObjectState` (and the bundle `makeTestState`) need
// not carry the API.
interface HibernatableState {
  acceptWebSocket(ws: WorkersWebSocket, tags?: string[]): void;
  getWebSockets(tag?: string): WorkersWebSocket[];
}

// Accept a server socket into the DO under a fresh connection id (the hibernation
// tag), attach the id for wake-time recovery, and wrap it. The returned
// `WorkersConnection` carries the connId a held store persists.
export function acceptHibernatableConnection<F>(
  state: DurableObjectState,
  server: WorkersWebSocket,
  // v0.106 (slice 3b-iii): when the service has an inbound/close handler, the
  // sender identity + route args are attached too, so a waking `webSocketMessage`/
  // `webSocketClose` recovers them (the `on open` already authenticated the actor).
  meta?: Record<string, unknown>,
): WorkersConnection<F> {
  const connId = crypto.randomUUID();
  (state as unknown as HibernatableState).acceptWebSocket(server, [connId]);
  server.serializeAttachment(meta === undefined ? { connId } : { connId, ...meta });
  return new WorkersConnection<F>(server, connId);
}

// Re-present the live socket a stored connId names, re-wrapped as a
// `WorkersConnection`. `None` when the platform has no such socket — a connection
// that has since closed (normal lifecycle, not corruption).
export function resolveConnection<F>(
  state: DurableObjectState,
  connId: string,
): Option<WorkersConnection<F>> {
  const sockets = (state as unknown as HibernatableState).getWebSockets(connId);
  const ws = sockets[0];
  return ws === undefined ? None : Some(new WorkersConnection<F>(ws, connId));
}

export interface WorkersWebSocketPair {
  client: WorkersWebSocket;
  server: WorkersWebSocket;
}

// Construct a Cloudflare `WebSocketPair` (a Workers runtime global), returned as
// a named `{ client, server }` pair. The pair is index-shaped at runtime
// (`pair[0]` = client, `pair[1]` = server); this normalises it and keeps the
// global access in one place so emitted code stays free of ambient declarations.
export function newWebSocketPair(): WorkersWebSocketPair {
  const Ctor = (globalThis as { WebSocketPair?: new () => { 0: WorkersWebSocket; 1: WorkersWebSocket } })
    .WebSocketPair;
  if (Ctor === undefined) {
    throw new Error("WebSocketPair is not available in this runtime");
  }
  const pair = new Ctor();
  return { client: pair[0], server: pair[1] };
}

// Build the `101 Switching Protocols` response that hands the client end of an
// accepted `WebSocketPair` back to the caller — the Cloudflare upgrade
// completion. `webSocket` is a Workers-specific `ResponseInit` extension.
export function webSocketUpgradeResponse(client: WorkersWebSocket): Response {
  return new Response(null, { status: 101, webSocket: client } as ResponseInit & {
    webSocket: WorkersWebSocket;
  });
}

// #1653: the `__` aliases emitted code imports.
export { WorkersConnection as __WorkersConnection, acceptHibernatableConnection as __acceptHibernatableConnection, connIdOf as __connIdOf, newWebSocketPair as __newWebSocketPair, resolveConnection as __resolveConnection, webSocketUpgradeResponse as __webSocketUpgradeResponse };
