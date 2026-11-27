// The WebSocket half of the browser-safe runtime `bynk client` inlines (#3906). It is added to a generated
// client only when the project has a `from websocket` service, so a client without one is byte-identical
// to what it was. Like `http_client.ts`, it is one module scope after splicing: the relative import below is
// dropped there.

import type { BoundaryError, CallFailure, ClientOptions, JsonValue, __Request } from "./http_client.ts";
import { __invalid } from "./http_client.ts";
import type { Result } from "../result.ts";

// ---------------------------------------------------------------------------
// WebSocket (#3906)
// ---------------------------------------------------------------------------

/** Why a WebSocket connection ended. */
export interface WsClose {
  /** The close frame's code: `1000` a normal close, `1008` a refused credential, `4008` a frame this client refused, `1006` an abnormal disconnection. */
  readonly code: number;
  readonly reason: string;
  /** Whether the close handshake completed. */
  readonly clean: boolean;
  /** Whether the client is about to reconnect (see `reconnect`). `false` on the last close. */
  readonly willReconnect: boolean;
}

/** Reconnection, which is off unless a `connect` asks for it. */
export interface WsReconnect<C> {
  /**
   * A credential for each attempt: how a connection outlives an expired token. Left out for a service that
   * takes none.
   */
  readonly credentials?: () => C | globalThis.Promise<C>;
  /** Attempts in a row before giving up. Default 5. */
  readonly maxAttempts?: number;
  /** Milliseconds to wait before attempt `n` (from 1). Default: 250 doubling each time, at most 30 000. */
  readonly backoff?: (attempt: number) => number;
}

/** What a `connect` takes after the route's parameters and the credentials. */
export interface WsConnectOptions<C> {
  /** The path the upgrade goes to. Default `/`: the Worker routes an upgrade by its header, not its path. */
  readonly path?: string;
  /**
   * Reconnect after the server closes the connection for an expired credential (`1008`, `authentication
   * expired`) or the connection drops (`1006`), with backoff, asking `credentials` for a fresh one each time.
   * A close the client made (`close()`, or a frame it refused) never reconnects. Frames sent or missed across a
   * reconnect are not replayed, and a frame queued before a close is dropped, never resent on the next
   * connection: the application decides what to resend. `credentials` is called once for every attempt.
   */
  readonly reconnect?: WsReconnect<C>;
}

/** The connection `connect` opens: typed frames both ways. */
export interface WsConnection<In, Out> {
  /**
   * Send a frame. It is encoded and checked against the type's refinements first: a value the server would
   * refuse returns `CallFailure` `Invalid` and nothing is sent. Before the connection is open the frame
   * waits for it; once the connection is closed or reconnecting it returns `CallFailure` `Transport`.
   * Returns `null` once sent or queued.
   */
  send(frame: In): CallFailure | null;
  /**
   * The frames the server sends, in order, decoded and checked as the server's `out` type, across
   * reconnects. Ends when the connection does. Frames are kept from the first read of this property until
   * they are read: read it only if you read it, or use `onFrame`.
   */
  readonly frames: globalThis.AsyncIterable<Out>;
  /** Call `listener` with each frame. Returns a function that stops it. */
  onFrame(listener: (frame: Out) => void): () => void;
  /** Call `listener` each time the connection closes, including a close that is followed by a reconnect. */
  onClose(listener: (close: WsClose) => void): () => void;
  /** Call `listener` with the attempt number each time a reconnect succeeds. */
  onReconnect(listener: (attempt: number) => void): () => void;
  /** Resolves when the connection first opens, and rejects if it closes before it did. */
  readonly opened: globalThis.Promise<void>;
  readonly state: "connecting" | "open" | "reconnecting" | "closed";
  /** Close the connection for good, with `1000` or a code from `3000` to `4999` (a page may use no other). No reconnect follows. */
  close(code?: number, reason?: string): void;
}

/** How a service takes its credential, and how its frames cross the wire. */
export interface __WsSpec<In, Out> {
  /** `bearer?` is a `Bearer` actor in a sum with an anonymous one: the token may be left out. */
  readonly credential: "none" | "bearer" | "bearer?" | "oidc" | "cookie";
  /** A plain `Bytes` frame is a binary message, anything else is JSON text (the server's own rule). */
  readonly binaryIn: boolean;
  readonly binaryOut: boolean;
  readonly encode: (frame: In) => Result<JsonValue | globalThis.Uint8Array, BoundaryError>;
  readonly decode: (frame: JsonValue | globalThis.Uint8Array) => Result<Out, BoundaryError>;
}

type __WsToken = string | (() => string | globalThis.Promise<string>);

/** The WebSocket URL of `path` under the client's `baseUrl`, with the route's parameters in its query string. */
function __wsUrl(options: ClientOptions, path: string, params: { readonly [name: string]: string }): string {
  let base = (options.baseUrl ?? "").replace(/\/+$/, "");
  if (base === "") {
    const where = (globalThis as { readonly location?: { readonly origin: string } }).location;
    if (where === undefined) throw new globalThis.Error("a WebSocket needs a `baseUrl` where there is no page to be relative to");
    base = where.origin;
  }
  base = base.replace(/^http(s?):/, "ws$1:");
  const query = new globalThis.URLSearchParams(params).toString();
  return base + path + (query === "" ? "" : "?" + query);
}

/** The subprotocols the upgrade offers: `["bynk.bearer", token]` or `["bynk.oidc", token]`, none for a cookie or anonymous one. */
async function __wsProtocols(spec: { readonly credential: string }, credentials: unknown): Promise<string[]> {
  const marker = spec.credential === "oidc" ? "bynk.oidc" : spec.credential.startsWith("bearer") ? "bynk.bearer" : undefined;
  if (marker === undefined) return [];
  const given = (credentials as { readonly bearer?: __WsToken; readonly oidc?: __WsToken } | undefined) ?? {};
  const token = marker === "bynk.oidc" ? given.oidc : given.bearer;
  if (token === undefined) {
    if (spec.credential === "bearer?") return [];
    throw new globalThis.Error("this service needs a credential");
  }
  return [marker, typeof token === "function" ? await token() : token];
}

class __WsClient<C, In, Out> implements WsConnection<In, Out> {
  state: "connecting" | "open" | "reconnecting" | "closed" = "connecting";
  readonly opened: globalThis.Promise<void>;
  private socket: globalThis.WebSocket | undefined;
  // The socket of the attempt in progress, until that attempt settles: what `close()` must close.
  private live: globalThis.WebSocket | undefined;
  private readonly credentials: C | undefined;
  private readonly queue: (string | globalThis.Uint8Array)[] = [];
  private readonly frameListeners = new globalThis.Set<(frame: Out) => void>();
  private readonly closeListeners = new globalThis.Set<(close: WsClose) => void>();
  private readonly reconnectListeners = new globalThis.Set<(attempt: number) => void>();
  private pending: Out[] | undefined;
  private waiting: ((r: IteratorResult<Out>) => void)[] = [];
  private attempt = 0;
  private everOpened = false;
  private ours = false;
  private finished = false;
  private timer: ReturnType<typeof globalThis.setTimeout> | undefined;
  private resolveOpened!: () => void;
  private rejectOpened!: (e: unknown) => void;

  private readonly options: ClientOptions;
  private readonly spec: __WsSpec<In, Out>;
  private readonly url: string;
  private readonly connect: WsConnectOptions<C> | undefined;

  constructor(
    options: ClientOptions,
    spec: __WsSpec<In, Out>,
    url: string,
    credentials: C | undefined,
    connect: WsConnectOptions<C> | undefined,
  ) {
    this.options = options;
    this.spec = spec;
    this.url = url;
    this.connect = connect;
    this.credentials = credentials;
    this.opened = new globalThis.Promise<void>((resolve, reject) => {
      this.resolveOpened = resolve;
      this.rejectOpened = reject;
    });
    // A caller who does not read `opened` must not see an unhandled rejection for a close it handles itself.
    this.opened.catch(() => undefined);
    void this.open(credentials);
  }

  get frames(): globalThis.AsyncIterable<Out> {
    this.pending ??= [];
    return {
      [globalThis.Symbol.asyncIterator]: (): AsyncIterator<Out> => {
        // Leaving the loop early (`break`) stops this iterator buffering: it reads nothing more.
        let left = false;
        return {
        next: (): globalThis.Promise<IteratorResult<Out>> => {
          if (left) return globalThis.Promise.resolve({ done: true, value: undefined });
          const have = this.pending!.shift();
          if (have !== undefined) return globalThis.Promise.resolve({ done: false, value: have });
          if (this.finished) return globalThis.Promise.resolve({ done: true, value: undefined });
          return new globalThis.Promise((resolve) => this.waiting.push(resolve));
        },
        return: (): globalThis.Promise<IteratorResult<Out>> => {
          left = true;
          this.pending = undefined;
          for (const waiter of this.waiting.splice(0)) waiter({ done: true, value: undefined });
          return globalThis.Promise.resolve({ done: true, value: undefined });
        },
        };
      },
    };
  }

  onFrame(listener: (frame: Out) => void): () => void {
    this.frameListeners.add(listener);
    return () => void this.frameListeners.delete(listener);
  }

  onClose(listener: (close: WsClose) => void): () => void {
    this.closeListeners.add(listener);
    return () => void this.closeListeners.delete(listener);
  }

  onReconnect(listener: (attempt: number) => void): () => void {
    this.reconnectListeners.add(listener);
    return () => void this.reconnectListeners.delete(listener);
  }

  send(frame: In): CallFailure | null {
    if (this.state === "closed" || this.state === "reconnecting") {
      return { kind: "Transport", cause: new globalThis.Error(`the connection is ${this.state}`) };
    }
    let wire: JsonValue | globalThis.Uint8Array;
    try {
      const encoded = this.spec.encode(frame);
      if (encoded.tag === "Err") return { kind: "Invalid", error: encoded.error };
      wire = encoded.value;
    } catch (e) {
      return __invalid(e);
    }
    const out = wire instanceof globalThis.Uint8Array ? wire : globalThis.JSON.stringify(wire);
    if (this.state === "connecting") this.queue.push(out);
    else this.socket!.send(out as globalThis.BufferSource | string);
    return null;
  }

  close(code: number = 1000, reason: string = ""): void {
    this.ours = true;
    if (this.timer !== undefined) globalThis.clearTimeout(this.timer);
    if (this.live !== undefined) {
      // An attempt is under way, or the connection is open: closing its socket ends it through its own `close` event.
      this.live.close(code, reason);
    } else {
      const info = { code, reason, clean: true };
      for (const l of [...this.closeListeners]) l({ ...info, willReconnect: false });
      this.end(info);
    }
  }

  private async open(credentials: C | undefined): Promise<void> {
    let socket: globalThis.WebSocket;
    try {
      const make = this.options.webSocket ?? (globalThis as { readonly WebSocket?: typeof globalThis.WebSocket }).WebSocket;
      if (make === undefined) throw new globalThis.Error("this environment has no WebSocket");
      const protocols = await __wsProtocols(this.spec, credentials);
      // `close()` while the token was being read: this attempt makes no socket.
      if (this.ours || this.finished) return;
      socket = protocols.length === 0 ? new make(this.url) : new make(this.url, protocols);
    } catch {
      this.closed({ code: 1006, reason: "", clean: false });
      return;
    }
    socket.binaryType = "arraybuffer";
    this.socket = socket;
    this.live = socket;
    // A browser fires `error` then `close` for an upgrade the server refuses, but some platforms (Node's
    // own `WebSocket`) fire only `error` and never `close`: an error before the socket ever opened is the
    // end of this attempt, and whichever of the two arrives second is ignored.
    let settled = false;
    let opened = false;
    const settle = (info: { readonly code: number; readonly reason: string; readonly clean: boolean }): void => {
      if (settled) return;
      settled = true;
      if (this.live === socket) this.live = undefined;
      this.closed(info);
    };
    socket.addEventListener("error", () => {
      if (!opened) settle({ code: 1006, reason: "", clean: false });
    });
    socket.addEventListener("open", () => {
      opened = true;
      this.state = "open";
      for (const queued of this.queue.splice(0)) socket.send(queued as globalThis.BufferSource | string);
      if (!this.everOpened) {
        this.everOpened = true;
        this.resolveOpened();
      } else {
        const attempt = this.attempt;
        for (const l of [...this.reconnectListeners]) l(attempt);
      }
      this.attempt = 0;
    });
    socket.addEventListener("message", (event: globalThis.MessageEvent) => this.receive(socket, event.data));
    socket.addEventListener("close", (event: globalThis.CloseEvent) =>
      settle({ code: event.code, reason: event.reason, clean: event.wasClean }),
    );
  }

  private receive(socket: globalThis.WebSocket, data: unknown): void {
    let frame: Out;
    try {
      let wire: JsonValue | globalThis.Uint8Array;
      if (this.spec.binaryOut) {
        if (!(data instanceof globalThis.ArrayBuffer)) throw new globalThis.Error("a text frame where a binary one is expected");
        wire = new globalThis.Uint8Array(data);
      } else {
        if (typeof data !== "string") throw new globalThis.Error("a binary frame where a text one is expected");
        wire = globalThis.JSON.parse(data) as JsonValue;
      }
      const decoded = this.spec.decode(wire);
      if (decoded.tag === "Err") throw new globalThis.Error("a frame the server's own type refuses");
      frame = decoded.value;
    } catch {
      // As the server does for a frame it cannot read: close. A page may not close with `1008` itself (the
      // platform allows `1000` and `3000` to `4999`), so the code is `4008`, the same number in the range it may use.
      this.ours = true;
      socket.close(4008, "invalid frame");
      return;
    }
    for (const l of [...this.frameListeners]) l(frame);
    if (this.pending !== undefined) {
      const waiter = this.waiting.shift();
      if (waiter !== undefined) waiter({ done: false, value: frame });
      else this.pending.push(frame);
    }
  }

  private closed(info: { readonly code: number; readonly reason: string; readonly clean: boolean }): void {
    // A frame queued while the connection was opening is not sent on a later one: a reconnect never resends
    // what was written before the close, which may or may not have reached the server.
    this.queue.length = 0;
    const reconnect = this.connect?.reconnect;
    const max = reconnect?.maxAttempts ?? 5;
    const retry =
      reconnect !== undefined &&
      !this.ours &&
      this.attempt < max &&
      (info.code === 1006 || (info.code === 1008 && info.reason === "authentication expired"));
    for (const l of [...this.closeListeners]) l({ ...info, willReconnect: retry });
    if (!retry) {
      this.end(info);
      return;
    }
    this.state = "reconnecting";
    this.attempt += 1;
    const delay = (reconnect!.backoff ?? ((n: number) => globalThis.Math.min(30000, 250 * 2 ** (n - 1))))(this.attempt);
    this.timer = globalThis.setTimeout(() => {
      void (async (): Promise<void> => {
        try {
          // Without `reconnect.credentials` the credentials `connect` was given are used again (a token that is a
          // function is read again): a reconnect never quietly becomes an anonymous connection.
          const fresh = (await reconnect!.credentials?.()) ?? this.credentials;
          if (this.ours || this.finished) return;
          await this.open(fresh);
        } catch {
          this.closed({ code: 1006, reason: "", clean: false });
        }
      })();
    }, delay);
  }

  private end(info: { readonly code: number; readonly reason: string; readonly clean: boolean }): void {
    if (this.finished) return;
    const wasConnecting = !this.everOpened;
    this.finished = true;
    this.state = "closed";
    if (wasConnecting) this.rejectOpened(new globalThis.Error(`closed before it opened (${info.code})`));
    for (const waiter of this.waiting.splice(0)) waiter({ done: true, value: undefined });
  }
}

/**
 * Open a WebSocket connection to a `from websocket` service: encode the route's parameters (the query string
 * of the upgrade), offer the credential as the server reads it, and wrap the socket with typed, checked frames.
 * Returns the connection, or the `CallFailure` that stopped it being made.
 */
export function __webSocket<C, In, Out>(
  options: ClientOptions,
  spec: __WsSpec<In, Out>,
  build: () => Result<__Request, BoundaryError>,
  credentials: C | undefined,
  connect: WsConnectOptions<C> | undefined,
): unknown {
  let request: __Request;
  try {
    const built = build();
    if (built.tag === "Err") return { kind: "Invalid", error: built.error } satisfies CallFailure;
    request = built.value;
  } catch (e) {
    return __invalid(e);
  }
  let url: string;
  try {
    url = __wsUrl(options, connect?.path ?? "/", request.params ?? {});
  } catch (cause) {
    return { kind: "Transport", cause } satisfies CallFailure;
  }
  return new __WsClient<C, In, Out>(options, spec, url, credentials, connect);
}
