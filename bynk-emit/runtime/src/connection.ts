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
  close(code?: number, reason?: string): Promise<void>;
}

// #3902: the close code and reason a `close(code?, reason?)` carries. An application code is `1000` or
// `3000`–`4999` (RFC 6455 reserves the rest, and `1004`, `1005`, `1006` and `1015` are never sent). A
// literal outside that is a compile error, so one reaching here was computed: it faults. A computed
// reason is cut to the protocol's 123 bytes of UTF-8 on a character boundary.
const CLOSE_REASON_MAX_BYTES = 123;

export function closeArguments(code?: number, reason?: string): { code?: number; reason?: string } {
  if (code === undefined) {
    return {};
  }
  if (!Number.isInteger(code) || (code !== 1000 && (code < 3000 || code > 4999))) {
    throw new Error(`Connection.close: ${code} is not a close code an application may send (1000, or 3000 to 4999)`);
  }
  if (reason === undefined) {
    return { code };
  }
  const encoder = new TextEncoder();
  if (encoder.encode(reason).length <= CLOSE_REASON_MAX_BYTES) {
    return { code, reason };
  }
  let cut = "";
  let bytes = 0;
  for (const ch of reason) {
    const n = encoder.encode(ch).length;
    if (bytes + n > CLOSE_REASON_MAX_BYTES) break;
    cut += ch;
    bytes += n;
  }
  return { code, reason: cut };
}

// v0.103: the bundle/test realisation of a `Connection[F]` — a capture-and-inspect
// channel. The runtime-managed lifecycle in production becomes an inspectable
// record in tests (design notes §20): `sent` holds every frame `send`-given, and
// `closed` records disposal. A test drives an `on open` handler with a
// `TestConnection` and asserts what the agent sent.
export class TestConnection<F> implements Connection<F> {
  readonly sent: F[] = [];
  closed = false;
  // #3902: the code and reason the server closed with, when it gave them.
  closeCode: number | undefined = undefined;
  closeReason: string | undefined = undefined;

  async send(frame: F): Promise<void> {
    if (this.closed) throw new Error("send on a closed TestConnection");
    this.sent.push(frame);
  }

  async close(code?: number, reason?: string): Promise<void> {
    const given = closeArguments(code, reason);
    this.closed = true;
    this.closeCode = given.code;
    this.closeReason = given.reason;
  }

  /** The code the server closed with, as `Option[Int]`: `None` before a close, or when it gave none. */
  closeCodeOption(): Option<number> {
    return this.closeCode === undefined ? None : Some(this.closeCode);
  }

  /** The reason the server closed with, as `Option[String]`. */
  closeReasonOption(): Option<string> {
    return this.closeReason === undefined ? None : Some(this.closeReason);
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
  private readonly ws: { send(data: string): void; close(code?: number, reason?: string): void };
  readonly connId: string;

  constructor(ws: { send(data: string): void; close(code?: number, reason?: string): void }, connId: string) {
    this.ws = ws;
    this.connId = connId;
  }

  async send(frame: F): Promise<void> {
    this.ws.send(JSON.stringify(frame));
  }

  async close(code?: number, reason?: string): Promise<void> {
    const given = closeArguments(code, reason);
    if (given.code === undefined) {
      this.ws.close();
    } else if (given.reason === undefined) {
      this.ws.close(given.code);
    } else {
      this.ws.close(given.code, given.reason);
    }
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

// #3902: what the Durable Object's `webSocketClose` and `webSocketError` run. A close frame whose code
// can be echoed gets the platform's recommended reply, `close(code, reason)`; one that cannot (`1004`,
// `1005`, `1006`, `1015`, or anything outside `1000`–`4999`; an abnormal disconnection has no frame to
// acknowledge) gets `close()` with no arguments. The reply is sent after the service's `on close` has
// run, even when it faults, and is harmless under `web_socket_auto_reply_to_close` (a closed socket's
// `close` is ignored, and a throw is swallowed here).
export interface WsCloseInfo {
  code: number;
  reason: string;
  clean: boolean;
}

interface WsClosingSocket {
  serializeAttachment(value: unknown): void;
  close(code?: number, reason?: string): void;
}

const UNSENDABLE_CLOSE_CODES: ReadonlySet<number> = new Set([1004, 1005, 1006, 1015]);

function echoable(code: number): boolean {
  return Number.isInteger(code) && code >= 1000 && code <= 4999 && !UNSENDABLE_CLOSE_CODES.has(code);
}

function completeClose(ws: WsClosingSocket, info: WsCloseInfo, reply: "echo" | "error"): void {
  try {
    if (reply === "error") {
      ws.close(1011, "error");
    } else if (echoable(info.code)) {
      ws.close(info.code, info.reason);
    } else {
      ws.close();
    }
  } catch {
    // Already closed: nothing left to acknowledge.
  }
}

/** Run `run(info)` once for this socket, then complete the close handshake. The attachment is marked
 * closed before the handler runs, so a later `webSocketClose` for the same socket replies without
 * running it again. */
export async function webSocketClosing(
  ws: WsClosingSocket,
  attachment: unknown,
  info: WsCloseInfo,
  run: (info: WsCloseInfo) => Promise<void>,
  reply: "echo" | "error" = "echo",
): Promise<void> {
  const state = (attachment ?? {}) as { closed?: boolean };
  try {
    if (state.closed !== true) {
      ws.serializeAttachment({ ...state, closed: true });
      await run(info);
    }
  } finally {
    completeClose(ws, info, reply);
  }
}

/** A non-disconnection error: log its class name (never a frame), then close with `1011` and run `on
 * close` with `{ code: 1011, reason: "error", clean: false }`. */
export async function webSocketErrored(
  ws: WsClosingSocket,
  attachment: unknown,
  error: unknown,
  run: (info: WsCloseInfo) => Promise<void>,
): Promise<void> {
  const name = error instanceof Error ? error.constructor.name : typeof error;
  console.error(`websocket error: ${name}`);
  await webSocketClosing(ws, attachment, { code: 1011, reason: "error", clean: false }, run, "error");
}

// #3903: whether the credential a connection was opened with has expired. The upgrade records the
// token's `exp` (seconds since the epoch) in the socket's attachment; a frame arriving after it closes
// the connection, so a connection cannot outlive its token's expiry (a hibernating connection can live
// for days). Only the expiry is re-checked: a revoked token, or one signed with a secret that has since
// been rotated, keeps acting until its own `exp`, which is why WebSocket tokens should be short-lived.
// Checked per frame, not by a timer: an idle expired connection can do nothing, and a timer would wake
// hibernating objects only to close it. The bound is the HTTP Bearer check's, `exp < now` with `now` in
// whole seconds as `verifyBearerJwtHs256` reads it, so a connection is never closed sooner than an HTTP
// request with the same token would be refused. A connection with no `exp` (a `None` upgrade, or one
// accepted before this was recorded) never expires.
export function webSocketAuthExpired(attachment: unknown, nowSeconds: number = Math.floor(Date.now() / 1000)): boolean {
  const exp = (attachment as { exp?: unknown } | null)?.exp;
  return typeof exp === "number" && exp < nowSeconds;
}
