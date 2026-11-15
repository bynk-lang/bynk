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
}

/** What one reply of a route is. */
export interface __Reply {
  readonly kind: string;
  /** What its body carries: a decoded value, `{ error: string }`, a `Location` header, a boundary error, or nothing. */
  readonly body: "value" | "message" | "location" | "boundary" | "none";
  readonly decode?: (json: JsonValue) => Result<unknown, BoundaryError>;
}

/** One route: its method, its path (`:name` is a parameter) and the replies it can make, by status. */
export interface __Route {
  readonly method: string;
  readonly path: string;
  readonly replies: { readonly [status: number]: readonly __Reply[] };
}

/** What one call sends, already encoded. */
export interface __Request {
  readonly params?: { readonly [name: string]: string };
  readonly query?: string;
  readonly body?: JsonValue;
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

/** A value that broke a boundary on its way out, as the `Invalid` failure it is. */
function __invalid(e: unknown): CallFailure {
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
  hasBody: boolean,
): Promise<globalThis.Headers> {
  const configured = typeof options.headers === "function" ? await options.headers() : options.headers;
  const headers = new globalThis.Headers(configured);
  new globalThis.Headers(init?.headers).forEach((value, name) => headers.set(name, value));
  if (hasBody && !headers.has("content-type")) headers.set("content-type", "application/json");
  if (options.origin !== undefined) headers.set("origin", options.origin);
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
  const hasBody = request.body !== undefined;
  let response: globalThis.Response;
  let text: string;
  try {
    const headers = await __headers(options, init, hasBody);
    const sent: globalThis.RequestInit = { ...init, method: route.method, headers };
    if (options.credentials !== undefined && sent.credentials === undefined) sent.credentials = options.credentials;
    if (hasBody) sent.body = globalThis.JSON.stringify(request.body);
    response = await send(__url(options, route, request), sent);
    text = await response.text();
  } catch (cause) {
    return { kind: "Transport", cause } satisfies CallFailure;
  }
  return __classify(route, response, text);
}
