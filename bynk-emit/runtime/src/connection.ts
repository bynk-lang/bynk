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

  // #3051: what a case's `svc.open(tc, …)` records so a later `svc.message(tc, …)`
  // / `svc.close(tc)` recover the route values and the principal from the
  // connection, as the platform recovers them from the socket attachment.
  private routeValues: unknown[] | undefined = undefined;
  private depsValue: unknown = undefined;
  private minted = false;

  /** Record `open`'s route values and `deps`; one socket is opened once, and not once it was handed out. */
  remember(route: unknown[], deps: unknown): void {
    if (this.routeValues !== undefined || this.minted) {
      throw new Error("TestConnection: this connection was already opened or handed to an agent — build a new TestConnection");
    }
    this.routeValues = route;
    this.depsValue = deps;
  }

  /** The route values and `deps` from `open`; a case that skipped `open` fails. */
  opened(): { route: unknown[]; deps: unknown } {
    if (this.routeValues === undefined) {
      throw new Error("TestConnection: `message`/`close` before `open` — call the service's `open` first");
    }
    return { route: this.routeValues, deps: this.depsValue };
  }

  /** The owned `Connection` an agent receives; one socket has one owner. */
  mint(): Connection<F> {
    if (this.minted || this.routeValues !== undefined) {
      throw new Error("TestConnection: this connection was already handed out or opened — one socket has one owner");
    }
    this.minted = true;
    return this;
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
//
// `protocol` is the subprotocol the response selects (#3900). A browser that offered subprotocols
// fails the connection when the response selects none, so a Bearer upgrade selects `bynk.bearer`.
// An upgrade that offered none is answered with none (the response is built as before).
export function webSocketUpgradeResponse(client: WorkersWebSocket, protocol?: string): Response {
  const init: ResponseInit & { webSocket: WorkersWebSocket } = { status: 101, webSocket: client };
  if (protocol !== undefined) {
    init.headers = { "Sec-WebSocket-Protocol": protocol };
  }
  return new Response(null, init);
}

// #3900: the subprotocol a Bearer WebSocket handshake selects. A browser cannot set a header on the
// upgrade, so it offers `new WebSocket(url, ["bynk.bearer", token])`: the marker, then the token. The
// response selects the marker only, so the credential is never echoed in a response header, where
// proxies and logs may record it.
export const BEARER_SUBPROTOCOL = "bynk.bearer";

// What a Bearer upgrade's `Sec-WebSocket-Protocol` offers: no token (`Absent`), a token after the
// marker (`Token`), or subprotocols without the marker (`Legacy`: the old `[token]` form, which only
// clients that do not enforce the standard could use).
export type WebSocketBearerOffer =
  | { readonly tag: "Absent" }
  | { readonly tag: "Legacy" }
  | { readonly tag: "Token"; readonly token: string };

export function webSocketBearerOffer(offered: string | null): WebSocketBearerOffer {
  if (offered === null) {
    return { tag: "Absent" };
  }
  const protocols = offered
    .split(",")
    .map((p) => p.trim())
    .filter((p) => p !== "");
  const at = protocols.indexOf(BEARER_SUBPROTOCOL);
  if (at === -1) {
    return protocols.length === 0 ? { tag: "Absent" } : { tag: "Legacy" };
  }
  const token = protocols[at + 1];
  return token === undefined ? { tag: "Absent" } : { tag: "Token", token };
}

// The subprotocol the 101 selects: the marker when the client offered it, else none.
export function webSocketBearerProtocol(offered: string | null): string | undefined {
  const marked = (offered ?? "").split(",").some((p) => p.trim() === BEARER_SUBPROTOCOL);
  return marked ? BEARER_SUBPROTOCOL : undefined;
}

// The refusal of the old single-element `[token]` form: `400`, and a fixed message naming the form
// that works (a `401` would hide the cause). It never echoes what the client sent.
export function webSocketBearerRefusal(): Response {
  return new Response('Offer the subprotocols ["bynk.bearer", <token>]', { status: 400 });
}
