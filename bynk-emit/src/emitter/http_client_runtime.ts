// The browser-safe runtime `bynk client` inlines into the module it generates.
// No signing, no Web Crypto, no secret and no Workers types: only `fetch`, which
// the caller may replace. The `Result`/`Option` helpers, the `Bytes` and
// `Decimal` codecs and the `Map`/`Set` keys come from the runtime's own modules,
// appended by the generator for just the names the codecs use.

/** A JSON value: what a codec reads and writes. */
export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [k: string]: JsonValue };

/** A refinement a value broke. */
export interface ValidationError {
  readonly field: string;
  readonly message: string;
  readonly value: unknown;
}

/** What a boundary refuses a value with: the server's `400` body, and what the client reports for a value it refuses to send. */
export type BoundaryError =
  | { readonly kind: "MalformedJson"; readonly details: string }
  | {
      readonly kind: "StructuralMismatch";
      readonly path: string;
      readonly expected: string;
      readonly actual: string;
    }
  | {
      readonly kind: "RefinementViolation";
      readonly path: string;
      readonly violation: ValidationError;
    }
  | { readonly kind: "Transport"; readonly status: number; readonly details: string };

/** The names the generated codecs use for them. */
export type __BoundaryError = BoundaryError;
export type __JsonValue = JsonValue;

/** Why a call returned no response of the route's own. Returned, never thrown. */
export type CallFailure =
  /** A value failed a refinement (or did not fit its type) before anything was sent. */
  | { readonly kind: "Invalid"; readonly error: BoundaryError }
  /** `fetch` rejected: a network failure, or an abort. */
  | { readonly kind: "Transport"; readonly cause: unknown }
  /** A status the route does not answer, or a body that no longer decodes: the sign of a stale client. */
  | {
      readonly kind: "Unexpected";
      readonly status: number;
      readonly text: string;
      readonly error?: BoundaryError;
    };

/** How to reach the API. Every field is optional. */
export interface ClientOptions {
  /** The API's origin and any path prefix, with no trailing slash needed. Default: relative URLs. */
  readonly baseUrl?: string;
  /** The `fetch` to call. Default: the global one. A framework's own (SvelteKit's `load` one) goes here. */
  readonly fetch?: typeof globalThis.fetch;
  /** Headers for every call: a value, or a (possibly async) function read per call, such as a bearer token. */
  readonly headers?:
    | globalThis.HeadersInit
    | (() => globalThis.HeadersInit | globalThis.Promise<globalThis.HeadersInit>);
  /** Passed to `fetch`: `"include"` sends cookies to another origin. */
  readonly credentials?: globalThis.RequestCredentials;
  /**
   * Sent as the `Origin` header. A browser sets its own and ignores this. Set it
   * when a framework's server calls a cookie-authenticated route that changes
   * state: the API admits such a request only from its own origin or a
   * credentialed CORS origin, and a server call carries neither by itself.
   */
  readonly origin?: string;
  /**
   * Send the route's contract hash as `X-Bynk-Contract`, so a server's log can tell which
   * contract a caller was generated for. Informational: nothing rejects a mismatch. Off by
   * default, because a custom header makes a cross-origin browser call preflighted, and the
   * server's `Access-Control-Allow-Headers` is only `content-type` (and `Authorization` on a
   * Bearer route) unless the service's `cors { headers: [...] }` lists `x-bynk-contract` (an
   * explicit list replaces the default).
   * A server-side call has no preflight and can turn it on freely.
   */
  readonly sendContract?: boolean;
}

/**
 * A Cloudflare Service Binding (`env.API`), or anything with a `fetch` method that takes
 * what `fetch` does.
 */
export interface FetchBinding {
  fetch(input: globalThis.RequestInfo | globalThis.URL, init?: globalThis.RequestInit): globalThis.Promise<globalThis.Response>;
}

/**
 * `ClientOptions` that send every call through a Service Binding instead of the network,
 * for a front end whose server runs on Cloudflare: `createClient({ ...viaBinding(env.API) })`.
 * The binding's `fetch` must be called as a method, so passing `env.API.fetch` itself
 * fails ("Illegal invocation"); this does it right. A binding ignores the host of the URL
 * it is given but needs a valid one, so `baseUrl` is an inert one unless you give another.
 * A binding call carries no `Origin` or `Sec-Fetch-Site`: a route that changes state under
 * a signed cookie needs `origin` too. It also sets `sendContract`, since a binding call has
 * no preflight to refuse the header (override it after the spread to turn it off).
 */
export function viaBinding(
  binding: FetchBinding,
  baseUrl: string = "https://bynk.invalid",
): { readonly baseUrl: string; readonly fetch: typeof globalThis.fetch; readonly sendContract: true } {
  return { baseUrl, sendContract: true, fetch: ((input, init) => binding.fetch(input, init)) as typeof globalThis.fetch };
}

/** What one reply of a route is. */
export interface __Reply {
  readonly kind: string;
  /** What its body carries: a decoded value, `{ error: string }`, a `Location` header, a boundary error, or nothing. */
  readonly body: "value" | "message" | "location" | "boundary" | "raw" | "events" | "none";
  /** A `raw` reply's `ETag`: `maybe` when the server may leave it out, `required` when it always sends one. */
  readonly etag?: "maybe" | "required";
  readonly decode?: (json: JsonValue) => Result<unknown, BoundaryError>;
}

/** One route: its method, its path (`:name` is a parameter) and the replies it can make, by status. */
export interface __Route {
  readonly method: string;
  readonly path: string;
  /** The hash of what this client acts on for the route (see `API_CONTRACTS`). */
  readonly contract?: string;
  readonly replies: { readonly [status: number]: readonly __Reply[] };
  /**
   * How a raw or streamed answer is read: set (to `__streamed`) only on a route that can
   * make one, so a client without such a route does not carry the reader.
   */
  readonly streamed?: (route: __Route, response: globalThis.Response) => unknown;
}

/** What one call sends, already encoded. */
export interface __Request {
  readonly params?: { readonly [name: string]: string };
  readonly query?: string;
  /** A JSON body. */
  readonly body?: JsonValue;
  /** A `@form` body: `application/x-www-form-urlencoded` text. */
  readonly form?: string;
  /** A `@multipart` body. */
  readonly multipart?: globalThis.FormData;
  /** The `Cookie` header's value. A browser sets its own and ignores this. */
  readonly cookies?: string;
  /** A `@raw` body: the caller's bytes, sent as given. */
  readonly raw?: globalThis.Uint8Array | globalThis.ArrayBuffer | globalThis.Blob | globalThis.ReadableStream<globalThis.Uint8Array>;
  /** The content type the route declared for its `@raw` body. */
  readonly rawType?: string;
}

/** The result of `f` over an `Ok`, an `Err` unchanged. */
export function __then<A, B, E>(r: Result<A, E>, f: (a: A) => B): Result<B, E> {
  return r.tag === "Ok" ? Ok(f(r.value)) : r;
}

/** Run `decode` over an encoded value: the same checks the server makes of it, so a refinement it would refuse is refused here. */
export function __checked(
  wire: JsonValue,
  decode: (json: JsonValue, path: string) => Result<unknown, BoundaryError>,
  at: string,
): Result<JsonValue, BoundaryError> {
  const r = decode(wire, at);
  return r.tag === "Ok" ? Ok(wire) : r;
}

/** `Some(x)` as `f(x)`, `None` as `null`. */
export function __optToNull<T, U>(v: Option<T>, f: (t: T) => U): U | null {
  return v.tag === "Some" ? f(v.value) : null;
}

/** `null` as `None`, anything else as `Some(f(x))`. */
export function __nullToOpt<T, U>(v: T | null | undefined, f: (t: T) => U): Option<U> {
  return v === null || v === undefined ? None : Some(f(v));
}

/** How one field of a `query` record is written: its wire name, and whether it is a list. */
export interface __QueryField {
  readonly name: string;
  readonly wire?: string;
}

/**
 * A `query` record's wire form as the query string the server reads: `?a=1&b=x&b=y`,
 * or `""` when every field is absent. A list repeats its key; an `Option` is its
 * value or nothing; a unit variant is its name.
 */
export function __queryString(wire: JsonValue, fields: readonly __QueryField[]): string {
  const params = new globalThis.URLSearchParams();
  const leaf = (v: JsonValue): string | undefined => {
    if (v === null) return undefined;
    if (typeof v === "object" && !globalThis.Array.isArray(v)) {
      if (v.kind === "None") return undefined;
      if (v.kind === "Some") return leaf(v.value ?? null);
      return typeof v.kind === "string" ? v.kind : undefined;
    }
    return globalThis.String(v);
  };
  if (typeof wire === "object" && wire !== null && !globalThis.Array.isArray(wire)) {
    for (const f of fields) {
      const value = wire[f.name];
      if (value === undefined) continue;
      for (const item of globalThis.Array.isArray(value) ? value : [value]) {
        const text = leaf(item);
        if (text !== undefined) params.append(f.wire ?? f.name, text);
      }
    }
  }
  const text = params.toString();
  return text === "" ? "" : `?${text}`;
}

/** A `form` body's text: the same as a query string without its `?`. */
export function __formString(wire: JsonValue, fields: readonly __QueryField[]): string {
  return __queryString(wire, fields).slice(1);
}

/** A value the client refuses to send, carrying the boundary error it is returned as. */
export class __Refusal extends globalThis.Error {
  readonly boundary: BoundaryError;
  constructor(boundary: BoundaryError) {
    super("refused before sending");
    this.boundary = boundary;
  }
}

// RFC 6265 §4.1.1, as the server's `serialiseCookie` reads them: a name is a token, a
// value is `cookie-octet`s.
const __COOKIE_NAME = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
const __COOKIE_VALUE = /^[\x21\x23-\x2B\x2D-\x3A\x3C-\x5B\x5D-\x7E]*$/;

/**
 * A `cookies` record's wire form as the `Cookie` header's value (`a=1; b=x; b=y`),
 * `""` when every field is absent. A cookie's value is not decoded, so it is
 * written raw, and a name or value that is not RFC 6265 text (a `;`, a space, a
 * quote) is refused: written raw it could add a cookie the caller never declared.
 */
export function __cookieString(wire: JsonValue, fields: readonly __QueryField[]): string {
  const query = __queryString(wire, fields);
  const pairs: string[] = [];
  new globalThis.URLSearchParams(query.slice(1)).forEach((value, name) => {
    if (!__COOKIE_NAME.test(name) || !__COOKIE_VALUE.test(value)) {
      throw new __Refusal({
        kind: "StructuralMismatch",
        path: `cookies.${name}`,
        expected: "an RFC 6265 cookie name and value (no `;`, `,`, space, quote, backslash or control character)",
        actual: value,
      });
    }
    pairs.push(`${name}=${value}`);
  });
  return pairs.join("; ");
}

/** The text parts of a `@multipart` body: `a=1&b=x` read back into `form`. */
export function __appendText(form: globalThis.FormData, text: string): void {
  new globalThis.URLSearchParams(text).forEach((value, name) => form.append(name, value));
}

/**
 * The file name a part is sent under: a `File`'s own, else `"blob"`, which is what a
 * browser names a bare `Blob`. A part with no name at all would be read back as text.
 */
export function __fileName(part: globalThis.Blob): string {
  const name = (part as { readonly name?: unknown }).name;
  return typeof name === "string" && name !== "" ? name : "blob";
}

/** A value that broke a boundary on its way out, as the `Invalid` failure it is. */
function __invalid(e: unknown): CallFailure {
  if (e instanceof __Refusal) return { kind: "Invalid", error: e.boundary };
  const details = e instanceof globalThis.Error ? e.message : globalThis.String(e);
  return {
    kind: "Invalid",
    error: { kind: "StructuralMismatch", path: "$", expected: "a value the wire can carry", actual: details },
  };
}

function __url(options: ClientOptions, route: __Route, request: __Request): string {
  const params = request.params ?? {};
  const path = route.path
    .split("/")
    .map((seg) => (seg.startsWith(":") ? globalThis.encodeURIComponent(params[seg.slice(1)] ?? "") : seg))
    .join("/");
  const base = (options.baseUrl ?? "").replace(/\/+$/, "");
  return base + path + (request.query ?? "");
}

async function __headers(
  options: ClientOptions,
  init: globalThis.RequestInit | undefined,
  request: __Request,
  route: __Route,
): Promise<globalThis.Headers> {
  const configured = typeof options.headers === "function" ? await options.headers() : options.headers;
  const headers = new globalThis.Headers(configured);
  new globalThis.Headers(init?.headers).forEach((value, name) => headers.set(name, value));
  // A form or multipart body names its own content type (a multipart one carries its
  // boundary), so a `content-type` configured for JSON is replaced, not kept. A JSON
  // body keeps one the caller chose (`application/vnd.api+json`).
  if (request.form !== undefined) headers.set("content-type", "application/x-www-form-urlencoded");
  else if (request.multipart !== undefined) headers.delete("content-type");
  else if (request.raw !== undefined) {
    // A `@raw` route names its content type: that, not one configured for JSON, is what it reads.
    if (request.rawType !== undefined) headers.set("content-type", request.rawType);
  } else if (request.body !== undefined && !headers.has("content-type")) headers.set("content-type", "application/json");
  // Typed cookies join a `Cookie` header the caller configured, they do not replace it.
  if (request.cookies !== undefined && request.cookies !== "") {
    const have = headers.get("cookie");
    headers.set("cookie", have === null || have === "" ? request.cookies : `${have}; ${request.cookies}`);
  }
  if (options.origin !== undefined) headers.set("origin", options.origin);
  if (options.sendContract === true && route.contract !== undefined) headers.set("x-bynk-contract", route.contract);
  return headers;
}

function __isObject(v: JsonValue): v is { [k: string]: JsonValue } {
  return typeof v === "object" && v !== null && !globalThis.Array.isArray(v);
}

/** The reply of `route` that `response` is, or why it is none. */
function __classify(route: __Route, response: globalThis.Response, text: string): unknown {
  const status = response.status;
  const unexpected = (error?: BoundaryError): CallFailure =>
    error === undefined ? { kind: "Unexpected", status, text } : { kind: "Unexpected", status, text, error };
  const replies = route.replies[status];
  if (replies === undefined) return unexpected();
  let json: JsonValue = null;
  let parsed = false;
  try {
    json = text === "" ? null : (globalThis.JSON.parse(text) as JsonValue);
    parsed = true;
  } catch {
    parsed = false;
  }
  let failure: BoundaryError | undefined;
  for (const reply of replies) {
    switch (reply.body) {
      case "none":
        return { status, kind: reply.kind };
      case "location": {
        const location = response.headers.get("location");
        if (location !== null) return { status, kind: reply.kind, location };
        break;
      }
      case "message":
        if (parsed && __isObject(json) && typeof json.error === "string") {
          return { status, kind: reply.kind, error: json.error };
        }
        break;
      case "boundary":
        if (parsed && __isObject(json) && typeof json.kind === "string") {
          return { status, kind: reply.kind, error: json };
        }
        break;
      case "value": {
        if (!parsed) break;
        const decoded = reply.decode === undefined ? Ok(json) : reply.decode(json);
        if (decoded.tag === "Ok") return { status, kind: reply.kind, body: decoded.value };
        failure = decoded.error;
        break;
      }
    }
  }
  return unexpected(failure);
}

/**
 * What iterating a call's `events` throws when the stream fails after its status was
 * sent (a dropped connection, an abort): the one place the client throws, because by
 * then there is no `CallFailure` left to return. `reason` is what the body rejected with.
 */
export class StreamFailure extends globalThis.Error {
  readonly reason: unknown;
  constructor(reason: unknown) {
    super("the event stream failed");
    this.name = "StreamFailure";
    this.reason = reason;
  }
}

/**
 * The events of a `text/event-stream` body, each one the `data` of an event as the
 * server wrote it. Lines end in CRLF, LF or CR; a `data` line adds a line to the event;
 * a blank line ends it; comments and every other field are ignored; an event the
 * stream ends in the middle of is dropped. Cancelling the iteration (a `break`, or
 * the call's `signal`) cancels the body. A body that fails throws a `StreamFailure` from the iteration.
 */
async function* __events(response: globalThis.Response): globalThis.AsyncGenerator<string, void, undefined> {
  if (response.body === null) return;
  const reader = response.body.getReader();
  const decoder = new globalThis.TextDecoder();
  let buffer = "";
  // Where the unread text may next hold a line end: what was scanned before holds none.
  let scanned = 0;
  let data: string[] = [];
  try {
    for (;;) {
      let chunk: globalThis.ReadableStreamReadResult<globalThis.Uint8Array>;
      try {
        chunk = await reader.read();
      } catch (reason) {
        throw new StreamFailure(reason);
      }
      const { value, done } = chunk;
      buffer += done ? decoder.decode() : decoder.decode(value, { stream: true });
      let start = 0;
      let held = -1;
      for (let i = scanned; i < buffer.length; i++) {
        const c = buffer[i];
        if (c !== "\n" && c !== "\r") continue;
        // A CR at the end of the text may be the first half of a CRLF: wait for more.
        if (c === "\r" && i + 1 === buffer.length && !done) {
          held = i;
          break;
        }
        const line = buffer.slice(start, i);
        if (c === "\r" && buffer[i + 1] === "\n") i++;
        start = i + 1;
        if (line === "") {
          if (data.length > 0) {
            const event = data.join("\n");
            data = [];
            yield event;
          }
        } else if (line[0] !== ":") {
          const colon = line.indexOf(":");
          const field = colon < 0 ? line : line.slice(0, colon);
          if (field === "data") {
            const rest = colon < 0 ? "" : line.slice(colon + 1);
            data.push(rest[0] === " " ? rest.slice(1) : rest);
          }
        }
      }
      scanned = (held < 0 ? buffer.length : held) - start;
      buffer = buffer.slice(start);
      if (done) return;
    }
  } finally {
    await reader.cancel().catch(() => undefined);
  }
}

/** The raw or streamed reply `response` is, if the route can answer so and the content type says it did. */
export function __streamed(route: __Route, response: globalThis.Response): unknown {
  const status = response.status;
  const replies = route.replies[status] ?? [];
  const essence = (response.headers.get("content-type") ?? "").split(";")[0]!.trim().toLowerCase();
  const events = replies.find((r) => r.body === "events");
  if (events !== undefined && essence === "text/event-stream") {
    return { status, kind: events.kind, events: __events(response) };
  }
  const raw = replies.find((r) => r.body === "raw");
  if (raw === undefined) return undefined;
  // A JSON body at a status that also answers in JSON is that answer; any other body is the author's.
  const json = replies.some((r) => r.body !== "raw" && r.body !== "events");
  if (json && essence === "application/json") return undefined;
  const reply: { [k: string]: unknown } = {
    status,
    kind: raw.kind,
    contentType: response.headers.get("content-type") ?? "",
  };
  if (raw.etag !== undefined) {
    const etag = response.headers.get("etag");
    // A tag the server always sends, missing: the call reads as a stale one, with the body's text.
    if (etag === null && raw.etag === "required") return undefined;
    reply.etag = etag;
  }
  reply.body = response.body;
  reply.bytes = async () => new globalThis.Uint8Array(await response.arrayBuffer());
  reply.text = () => response.text();
  return reply;
}

/** Make one call: encode, send, classify. Returns the route's reply, or the `CallFailure`. */
export async function __call(
  options: ClientOptions,
  init: globalThis.RequestInit | undefined,
  route: __Route,
  build: () => Result<__Request, BoundaryError>,
): Promise<unknown> {
  let request: __Request;
  try {
    const built = build();
    if (built.tag === "Err") return { kind: "Invalid", error: built.error } satisfies CallFailure;
    request = built.value;
  } catch (e) {
    return __invalid(e);
  }
  // A browser manages its own cookies and drops a `Cookie` header a script sets, so
  // a call that would silently lose its typed cookies is refused instead. A page
  // sends its cookies with `credentials: "include"`.
  if (
    request.cookies !== undefined &&
    request.cookies !== "" &&
    typeof (globalThis as { readonly document?: unknown }).document !== "undefined"
  ) {
    return {
      kind: "Invalid",
      error: {
        kind: "StructuralMismatch",
        path: "cookies",
        expected: "no typed cookies in a browser: it sets its own (pass `credentials: \"include\"`)",
        actual: request.cookies,
      },
    } satisfies CallFailure;
  }
  // `.` and `..` survive percent-encoding and a URL parser collapses them (as `%2e`
  // too), which would send the call to another route: refuse them as the value they
  // are, never as a path.
  for (const [name, value] of globalThis.Object.entries(request.params ?? {})) {
    if (value === "." || value === "..") {
      return {
        kind: "Invalid",
        error: { kind: "StructuralMismatch", path: `path.${name}`, expected: "a path segment", actual: value },
      } satisfies CallFailure;
    }
  }
  const send = options.fetch ?? globalThis.fetch;
  let response: globalThis.Response;
  let text: string;
  let streamed: unknown;
  try {
    const headers = await __headers(options, init, request, route);
    const sent: globalThis.RequestInit & { duplex?: "half" } = { ...init, method: route.method, headers };
    if (options.credentials !== undefined && sent.credentials === undefined) sent.credentials = options.credentials;
    if (request.body !== undefined) sent.body = globalThis.JSON.stringify(request.body);
    else if (request.form !== undefined) sent.body = request.form;
    else if (request.multipart !== undefined) sent.body = request.multipart;
    else if (request.raw !== undefined) {
      sent.body = request.raw as globalThis.BodyInit;
      // A streamed upload needs `duplex: "half"`, and only some browsers can make one.
      if (request.raw instanceof globalThis.ReadableStream) sent.duplex = "half";
    }
    response = await send(__url(options, route, request), sent);
    streamed = route.streamed?.(route, response);
    text = streamed === undefined ? await response.text() : "";
  } catch (cause) {
    return { kind: "Transport", cause } satisfies CallFailure;
  }
  return streamed !== undefined ? streamed : __classify(route, response, text);
}
