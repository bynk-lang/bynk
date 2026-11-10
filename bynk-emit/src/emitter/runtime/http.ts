import type { JsonValue, BoundaryError } from "./boundary.ts";
import type { Result, Option } from "./result.ts";
import type { HttpResult } from "./httpresult.ts";

// Match a path pattern (e.g., "/orders/:id") against a request path.
// Returns the captured parameter map, or null on no match.
export function matchPath(
  pattern: string,
  path: string,
): { params: Record<string, string> } | null {
  const patternSegments = pattern.split("/").filter(Boolean);
  const pathSegments = path.split("/").filter(Boolean);
  if (patternSegments.length !== pathSegments.length) return null;
  const params: Record<string, string> = {};
  for (let i = 0; i < patternSegments.length; i++) {
    const p = patternSegments[i];
    if (p.startsWith(":")) {
      params[p.slice(1)] = decodeURIComponent(pathSegments[i]);
    } else if (p !== pathSegments[i]) {
      return null;
    }
  }
  return { params };
}

// The HTTP status code each HttpResult variant maps to. Kept in sync with
// HTTP_VARIANTS in bynk-syntax/src/ast.rs (the compiler-side source of truth).
// `RawError` carries its own status, so it has no row.
const HTTP_STATUS: Record<Exclude<HttpResult<unknown>["tag"], "RawError">, number> = {
  Ok: 200,
  Streaming: 200,
  Raw: 200,
  RawTagged: 200,
  RawStreamed: 200,
  RawCreated: 201,
  Created: 201,
  Accepted: 202,
  NoContent: 204,
  MovedPermanently: 301,
  Found: 302,
  SeeOther: 303,
  TemporaryRedirect: 307,
  PermanentRedirect: 308,
  BadRequest: 400,
  Unauthorized: 401,
  Forbidden: 403,
  NotFound: 404,
  MethodNotAllowed: 405,
  NotAcceptable: 406,
  RequestTimeout: 408,
  Conflict: 409,
  Gone: 410,
  LengthRequired: 411,
  PayloadTooLarge: 413,
  UnsupportedMediaType: 415,
  UnprocessableEntity: 422,
  TooManyRequests: 429,
  UnavailableForLegalReasons: 451,
  ServerError: 500,
  NotImplemented: 501,
  BadGateway: 502,
  ServiceUnavailable: 503,
  GatewayTimeout: 504,
};

// Serialise an HttpResult<T> to a Response. The variant determines the HTTP
// status code; success variants carry the value as JSON, redirects emit a
// Location header, error variants carry an `{ error }` body, and the remaining
// statuses are bodyless.
// Frame a stream of event payloads as an SSE (`text/event-stream`) Response.
// Each stream element is one SSE event; a multi-line element becomes multiple
// `data:` lines, terminated by a blank line. The body is a ReadableStream, so
// this is a Web standard that runs unchanged on Workers and Node.
function sseResponse(stream: AsyncIterable<string>): Response {
  const encoder = new TextEncoder();
  const body = pullBody(stream, (value) => [
    ...value.split("\n").map((line) => encoder.encode(`data: ${line}\n`)),
    encoder.encode("\n"),
  ]);
  return new Response(body, {
    status: 200,
    headers: { "content-type": "text/event-stream", "cache-control": "no-cache" },
  });
}

// K-streams (#2623): mark `stream` as read, or throw if it already was. A
// stream is single-pass; the compiler's inline stream ops set the same mark
// (`STREAM_CLAIM` in `bynk-emit/src/emitter/lower.rs`), under a registered
// symbol so it holds across modules. Emitted code never imports this: it is
// exported for the platform bindings that will hand out streams (Fetch, K5c;
// R2, P5b) and for the runtime's own tests.
const STREAM_READ = Symbol.for("bynk.stream.read");
export function claimStream(stream: object): void {
  const mark = stream as Record<symbol, boolean>;
  if (mark[STREAM_READ]) {
    throw new Error("bynk: a stream can be read only once");
  }
  mark[STREAM_READ] = true;
}

// A lazy response body over `stream`, each element written as the chunks
// `encode` makes of it. The stream is claimed now: handing it to a response
// consumes it, whether or not the body is ever read.
// v0.139 (ADR 0162): a `pull` source with `highWaterMark: 0` makes the body
// lazy — the source stream is advanced only when a reader pulls, never
// eagerly at construction. So a streamed `GET` answered as `HEAD` never
// advances the source at all, and a real `GET` streams under the consumer's
// backpressure rather than buffering the whole stream up front. `cancel`
// returns the iterator so an abandoned read releases the source promptly. A
// throw from the source errors the body, aborting a response whose status is
// already sent.
function pullBody<T>(
  stream: AsyncIterable<T>,
  encode: (value: T) => readonly Uint8Array[],
): ReadableStream<Uint8Array> {
  claimStream(stream);
  const iterator = stream[Symbol.asyncIterator]();
  return new ReadableStream<Uint8Array>(
    {
      // An element that encodes to nothing (an empty chunk) is skipped
      // here: a `pull` that enqueues nothing isn't called again.
      async pull(controller) {
        for (;;) {
          const { value, done } = await iterator.next();
          if (done) {
            controller.close();
            return;
          }
          const chunks = encode(value);
          for (const chunk of chunks) {
            controller.enqueue(chunk);
          }
          if (chunks.length > 0) {
            return;
          }
        }
      },
      async cancel(reason) {
        await iterator.return?.(reason);
      },
    },
    { highWaterMark: 0 },
  );
}

// K5c (#2627): a lazy byte body over a `Stream[Bytes]`, claiming it now.
// `RawStreamed` writes its response with it, and the Fetch bindings their
// `sendStream` request body, so the empty-chunk skip lives in one place.
export function byteStreamBody(stream: AsyncIterable<Uint8Array>): ReadableStream<Uint8Array> {
  return pullBody(stream, (chunk) => (chunk.byteLength === 0 ? [] : [chunk]));
}

// #2858: a plain-JSON route's body serialiser. The generated `Option` encoder
// reads `__bynkPlainJson` and writes `Some(v)` as `v` and `None` as `null`
// instead of the tagged object; this sets it for the one synchronous
// serialisation and restores it, so a tagged route in the same Worker (and
// every cross-context, queue and agent-state codec) is unaffected. The flag
// lives on `globalThis`, so serialisation MUST stay synchronous: an `await`
// between set and restore would leak it into a concurrent request. Generic
// `Codable` witnesses (`codec.ts`) always write the tagged form and are out of
// scope: a route body is serialised by its monomorphic generated encoder.
export function withPlainJson<T>(serialise: (v: T) => JsonValue): (v: T) => JsonValue {
  return (v) => {
    const g = globalThis as { __bynkPlainJson?: boolean };
    const prev = g.__bynkPlainJson;
    g.__bynkPlainJson = true;
    try {
      return serialise(v);
    } finally {
      g.__bynkPlainJson = prev;
    }
  };
}

export function httpResultToResponse<T>(
  result: HttpResult<T>,
  serialiseValue: (v: T) => JsonValue,
  opts?: { readonly weakEtag?: boolean },
): Response {
  // #2341 (L4): an author-chosen error status with an author-owned body.
  if (result.tag === "RawError") {
    return new Response(result.body as BodyInit, {
      status: result.status,
      headers: { "content-type": result.contentType },
    });
  }
  const status = HTTP_STATUS[result.tag];
  switch (result.tag) {
    // 2xx with a body — the serialised value as JSON.
    case "Ok":
    case "Created":
    case "Accepted": {
      const body = JSON.stringify(serialiseValue(result.value));
      const headers: Record<string, string> = { "content-type": "application/json" };
      // v0.140 (ADR 0163): an eligible `GET` `Ok` carries a weak validator over its
      // serialised body, so a conditional re-fetch can be answered `304`. Of the
      // JSON variants only `Ok` (the success read) is eligible (DECISION B) — a
      // small `Raw` body is the other automatic case, below — and only when the caller
      // opts in — the `GET` dispatch site passes `weakEtag`, so a POST/PUT `Ok` (or
      // any other method) stays byte-for-byte unchanged.
      if (opts?.weakEtag && result.tag === "Ok") {
        headers["etag"] = weakETag(body);
      }
      return new Response(body, { status, headers });
    }
    // 200 with a streamed body — each stream element is one SSE event.
    case "Streaming":
      return sseResponse(result.stream);
    // 200 with a raw body — author-owned bytes with an explicit content-type.
    // No codec runs; the Uint8Array is written straight into the Response. The
    // `as BodyInit` cast satisfies TS 5.7, whose `Uint8Array<ArrayBufferLike>`
    // no longer matches DOM's `BufferSource` (it excludes SharedArrayBuffer) —
    // a bare Uint8Array is a valid body at runtime on both Workers and Node.
    // Caching raw responses: on a `GET` (the `weakEtag` opt-in), a body of at most
    // `RAW_WEAK_ETAG_MAX_BYTES` carries a weak validator over its octets, as a JSON
    // `Ok` does; a larger one carries none, so the hash never costs more than a
    // bounded slice of the Worker's CPU. A large body gets its validator from the
    // handler instead, through `RawTagged`.
    case "Raw": {
      const headers: Record<string, string> = { "content-type": result.contentType };
      if (opts?.weakEtag && result.body.byteLength <= RAW_WEAK_ETAG_MAX_BYTES) {
        headers["etag"] = weakETagBytes(result.body);
      }
      return new Response(result.body as BodyInit, { status: 200, headers });
    }
    // 200 with a raw body and the handler's own validator, sent as the `ETag`
    // without hashing, whatever the body's size and method. A malformed tag throws,
    // so the dispatch's fault path answers `500` and logs the route — never a
    // malformed header on the wire.
    case "RawTagged":
      if (!isEntityTag(result.etag)) {
        throw new Error(
          `RawTagged: ${JSON.stringify(result.etag)} is not an entity tag ` +
            `(RFC 9110 §8.8.3: quoted, optionally W/-prefixed, e.g. "v1" or W/"v1")`,
        );
      }
      return new Response(result.body as BodyInit, {
        status: 200,
        headers: { "content-type": result.contentType, etag: result.etag },
      });
    // K-streams (#2623): 200 with a raw body streamed chunk by chunk. No
    // validator, whatever `weakEtag` says: the bytes can't be hashed without
    // buffering them. An empty chunk is skipped, since it writes nothing.
    case "RawStreamed":
      return new Response(byteStreamBody(result.body), {
        status: 200,
        headers: { "content-type": result.contentType },
      });
    // 201 with a raw body — `Raw`'s writer at the creation status (#2403, L6).
    // It answers a write, so it carries no validator, whatever `weakEtag` says.
    case "RawCreated":
      return new Response(result.body as BodyInit, {
        status: 201,
        headers: { "content-type": result.contentType },
      });
    // 3xx — bodyless, with the target URL in the Location header.
    case "MovedPermanently":
    case "Found":
    case "SeeOther":
    case "TemporaryRedirect":
    case "PermanentRedirect":
      return new Response(null, { status, headers: { location: result.location } });
    // Error variants carrying an explanatory message — `{ error }` JSON body.
    case "BadRequest":
    case "Conflict":
    case "PayloadTooLarge":
    case "UnsupportedMediaType":
    case "UnprocessableEntity":
    case "TooManyRequests":
    case "UnavailableForLegalReasons":
    case "ServerError":
    case "NotImplemented":
    case "BadGateway":
    case "ServiceUnavailable":
    case "GatewayTimeout":
      return new Response(JSON.stringify({ error: result.message }), {
        status,
        headers: { "content-type": "application/json" },
      });
    // Self-describing statuses — bodyless.
    case "NoContent":
    case "Unauthorized":
    case "Forbidden":
    case "NotFound":
    case "MethodNotAllowed":
    case "NotAcceptable":
    case "RequestTimeout":
    case "Gone":
    case "LengthRequired":
      return new Response(null, { status });
  }
}

// v0.139 (ADR 0162): rebuild a `GET` response as its `HEAD` answer — identical
// status and headers, empty body (RFC 9110 §9.3.2). `Response.body` is not
// re-read: passing `null` discards the original body without consuming it, so a
// `Streaming` (`SSE`) `GET` answered as `HEAD` returns the stream's headers while
// its `ReadableStream` is never started or drained. `content-length` is omitted
// (the body is never materialised — permitted, §9.3.2 "MAY").
//
// K-streams (#2623): the body is cancelled rather than dropped, so a streamed
// body's source is returned (`pullBody`'s `cancel`), releasing a platform
// stream underneath it.
export function headResponse(response: Response): Response {
  response.body?.cancel().catch(() => {});
  return new Response(null, { status: response.status, headers: response.headers });
}

// testing-the-boundary Slice B: the inverse of `httpResultToResponse` for a
// `system`-tier test. A case drives a route with a real `fetch` and asserts on
// the returned `HttpResult[T]` (`expect item is Created(_)`), so the harness
// decodes the `Response` back through this. Status is the discriminator: it
// picks the canonical variant for each code (a 200 body decodes to `Ok`;
// `Streaming`/`Raw` — both 200 — are not distinguished, nor `RawCreated` from
// `Created`, since a test asserting on those would use the unit tier).
// Value-bearing 2xx parse the JSON body through `deserialiseValue` (a 201 that
// is not JSON keeps its text); error variants recover their `{ error }`
// message; redirects recover the `Location` header; the rest are bodyless.
const STATUS_TO_TAG: Record<number, HttpResult<unknown>["tag"]> = {
  200: "Ok",
  201: "Created",
  202: "Accepted",
  204: "NoContent",
  301: "MovedPermanently",
  302: "Found",
  303: "SeeOther",
  307: "TemporaryRedirect",
  308: "PermanentRedirect",
  400: "BadRequest",
  401: "Unauthorized",
  403: "Forbidden",
  404: "NotFound",
  405: "MethodNotAllowed",
  406: "NotAcceptable",
  408: "RequestTimeout",
  409: "Conflict",
  410: "Gone",
  411: "LengthRequired",
  413: "PayloadTooLarge",
  415: "UnsupportedMediaType",
  422: "UnprocessableEntity",
  429: "TooManyRequests",
  451: "UnavailableForLegalReasons",
  500: "ServerError",
  501: "NotImplemented",
  502: "BadGateway",
  503: "ServiceUnavailable",
  504: "GatewayTimeout",
};

// #2658: `bynk test --smoke` sets `BYNK_TEST_STRICT_WIRE` in the runner's
// environment, so an answer outside the route's contract fails the case where
// an author's own case gets the raw value to judge. Read in the test runner
// only: no Worker's environment sets it.
function strictWire(): boolean {
  const host = globalThis as { process?: { env?: Record<string, string | undefined> } };
  return host.process?.env?.BYNK_TEST_STRICT_WIRE === "1";
}

// #2685: `bynk test --smoke` hands the runner the statuses each route's handler
// can answer (`BYNK_SMOKE_DECLARED_FILE`: the path of a JSON file holding an object from a route's key, which the
// driver names `<context> <service> <METHOD> <path>`, to its statuses). A smoke
// case's typed call passes its key to `responseToHttpResult`, and a non-5xx
// answer outside that set is a warning, never a failure: the set is walked from
// the handler's body, and treating it as exhaustive would fail on exactly the
// false positives the warning exists to avoid. A `5xx` is the case's own
// failure, and a route the table does not hold is not checked. Under `ndjson`
// the warning is a line of the event stream, which the driver attaches to the
// case that follows; otherwise it is a line on stderr.
const SMOKE_UNDECLARED_STATUS = "bynk.test.smoke_undeclared_status";
let smokeParsed: { raw: string; table: Record<string, number[] | undefined> } | undefined;

function smokeNote(route: string, status: number): void {
  const host = globalThis as {
    process?: {
      env?: Record<string, string | undefined>;
      getBuiltinModule?(id: string): { readFileSync(path: string, encoding: string): string } | undefined;
    };
    console?: { log(line: string): void; error(line: string): void };
  };
  const file = host.process?.env?.BYNK_SMOKE_DECLARED_FILE;
  if (file === undefined) return;
  let parsed = smokeParsed;
  if (parsed === undefined || parsed.raw !== file) {
    try {
      const text = host.process?.getBuiltinModule?.("node:fs")?.readFileSync(file, "utf8") ?? "{}";
      parsed = { raw: file, table: JSON.parse(text) };
    } catch {
      parsed = { raw: file, table: {} };
    }
    smokeParsed = parsed;
  }
  const declared = parsed.table[route];
  if (declared === undefined || status >= 500 || declared.includes(status)) return;
  const [context, service, method, ...rest] = route.split(" ");
  const path = rest.join(" ");
  const named = STATUS_TO_TAG[status];
  const message =
    `\`${method} ${path}\` (${context}, service \`${service}\`) answered ${status}` +
    `${named === undefined ? "" : ` ${named}`}, which it does not declare; it declares ${declared.join(", ")}`;
  if (host.process?.env?.BYNK_TEST_FORMAT === "ndjson") {
    host.console?.log(
      JSON.stringify({
        type: "warning",
        code: SMOKE_UNDECLARED_STATUS,
        route: `${method} ${path}`,
        status,
        declared,
        message,
      }),
    );
  } else {
    host.console?.error(`warning[${SMOKE_UNDECLARED_STATUS}]: ${message}`);
  }
}

export async function responseToHttpResult<T>(
  response: Response,
  deserialiseValue: (json: JsonValue) => Result<T, BoundaryError>,
  route?: string,
): Promise<HttpResult<T>> {
  if (route !== undefined && strictWire()) smokeNote(route, response.status);
  const known = STATUS_TO_TAG[response.status];
  if (known === undefined && strictWire()) {
    throw new globalThis.Error(`the answer's status ${response.status} is not one an \`HttpResult\` carries`);
  }
  const tag = known ?? "ServerError";
  switch (tag) {
    // A 201 may be a `RawCreated` (#2403, L6): it still reads as `Created`,
    // with the body's text as the value when it does not parse. A 200 with a
    // content type that is neither JSON nor an event stream reads as `Raw`
    // (#3050, below); only a 200 with no content type falls through to `Ok`
    // with the body's text. A 200 or 202 that says it is JSON must parse, so a
    // handler whose JSON breaks still fails its case.
    case "Ok":
    case "Accepted":
    case "Created": {
      // #3050: a 200 whose content type is neither JSON nor an event stream is
      // a `Raw` (or `RawTagged`/`RawStreamed`, which the wire cannot tell
      // apart): surface the unit tier's `Raw(body, contentType)`, the type
      // verbatim as the handler set it. A raw body that *says* JSON still
      // reads as `Ok`, as it cannot be told from a JSON value.
      if (tag === "Ok") {
        const ct = response.headers.get("content-type");
        const essence = (ct ?? "").split(";")[0].trim().toLowerCase();
        if (ct !== null && essence !== "" && essence !== "application/json" && essence !== "text/event-stream") {
          const body = new Uint8Array(await response.arrayBuffer());
          return { tag: "Raw", body, contentType: ct } as unknown as HttpResult<T>;
        }
      }
      const text = await response.text();
      const saysJson =
        (response.headers.get("content-type") ?? "").split(";")[0].trim().toLowerCase() ===
        "application/json";
      let json: JsonValue;
      try {
        json = JSON.parse(text) as JsonValue;
      } catch (e) {
        if (tag !== "Created" && saysJson) {
          throw e;
        }
        return { tag, value: text as unknown as T };
      }
      const decoded = deserialiseValue(json);
      if (decoded.tag !== "Ok" && saysJson && strictWire()) {
        throw new globalThis.Error(
          `the answer's ${response.status} body does not decode as the route's declared type: ${JSON.stringify(decoded.error)}`,
        );
      }
      const value = (decoded.tag === "Ok" ? decoded.value : (json as unknown)) as T;
      return { tag, value };
    }
    case "MovedPermanently":
    case "Found":
    case "SeeOther":
    case "TemporaryRedirect":
    case "PermanentRedirect":
      return { tag, location: response.headers.get("location") ?? "" };
    case "BadRequest":
    case "Conflict":
    case "PayloadTooLarge":
    case "UnsupportedMediaType":
    case "UnprocessableEntity":
    case "TooManyRequests":
    case "UnavailableForLegalReasons":
    case "ServerError":
    case "NotImplemented":
    case "BadGateway":
    case "ServiceUnavailable":
    case "GatewayTimeout": {
      let message = "";
      try {
        const body = (await response.json()) as { error?: string };
        message = body.error ?? "";
      } catch {
        message = "";
      }
      return { tag, message } as HttpResult<T>;
    }
    default:
      // NoContent and the self-describing bodyless statuses.
      return { tag } as HttpResult<T>;
  }
}

// Slice C (testing-the-boundary): the outcome of a `system`-tier address driven
// with a raw `Wire(…)` argument. A boundary rejection never produces an
// `HttpResult` (the handler never ran), so a `Wire`-carrying call yields this
// sum instead: `Rejected(detail)` when the router refused the input before the
// handler, or `Handled(httpResult)` when it ran. The rejection `detail` carries a
// `tag` mirroring the router's `BoundaryError.kind` (`RefinementViolation`,
// `MalformedJson`, `StructuralMismatch`), so `expect r is Rejected(
// RefinementViolation(_))` lowers to a plain `.tag` test. #1934: the checker
// types it as `HttpOutcome[T]`, and every rejection carries `detail`: for a body
// or path rejection the router's description of what it refused, which
// `RefinementViolation(d)` binds (`BOUNDARY_REJECTIONS` in
// `bynk-check/src/checker.rs`), and `""` for the seam's and router's refusals.
export type HttpOutcome<T> =
  | {
      readonly tag: "Rejected";
      readonly value: { readonly tag: string; readonly detail: string; readonly [k: string]: unknown };
    }
  | { readonly tag: "Handled"; readonly value: HttpResult<T> };

// #1934: the one-line description a `Rejected` body or path rejection carries
// as `detail`, from the router's `BoundaryError` body.
function rejectionDetail(body: Record<string, unknown>): string {
  const str = (v: unknown): string => (typeof v === "string" ? v : "");
  switch (body.kind) {
    case "MalformedJson":
    case "MalformedForm":
    case "PayloadTooLarge":
    case "UnsupportedMediaType":
      return str(body.details);
    case "StructuralMismatch":
      return `${str(body.path)}: expected ${str(body.expected)}, got ${str(body.actual)}`;
    case "RefinementViolation": {
      const violation = body.violation as { message?: unknown } | undefined;
      return `${str(body.path)}: ${str(violation?.message)}`;
    }
    default:
      return "";
  }
}

// #2341 (decision C): the rejection a router-synthesised response stands for,
// read from its status and body — what `on rejected` receives. Only the
// router's own refusals reach this (a handler's responses never do), so a `401`
// or `405` here is the seam's or the router's. `null` for anything else (the
// `204` to `OPTIONS`).
async function rejectionOf(
  response: Response,
): Promise<{ readonly tag: string; readonly detail: string } | null> {
  if (response.status === 401) return { tag: "Unauthorized", detail: "" };
  if (response.status === 405) return { tag: "MethodNotAllowed", detail: "" };
  if (response.status !== 400 && response.status !== 413 && response.status !== 415) return null;
  let body: Record<string, unknown> | null = null;
  try {
    body = (await response.clone().json()) as Record<string, unknown>;
  } catch {
    body = null;
  }
  if (body === null || typeof body.kind !== "string") return null;
  return { tag: body.kind, detail: rejectionDetail(body) };
}

// #2341 (decision C): render a router rejection through a service's
// `on rejected`. The status is the rejection's own, so the hook's answer is
// used only when it is a `RawError` of that status; anything else — another
// variant, another status, or a fault in the hook — falls back to the router's
// default response, and is reported. The default's other headers (`Allow` on
// a `405`) are kept.
export async function renderRejection(
  response: Response,
  hook: (why: { readonly tag: string; readonly detail: string }) => Promise<HttpResult<unknown>>,
): Promise<Response> {
  const why = await rejectionOf(response);
  if (why === null) return response;
  let result: HttpResult<unknown>;
  try {
    result = await hook(why);
  } catch (e) {
    console.error(`bynk: \`on rejected\` faulted rendering a ${response.status}; the default response was sent`, e);
    return response;
  }
  if (result.tag !== "RawError" || result.status !== response.status) {
    const got = result.tag === "RawError" ? `RawError(${result.status}, …)` : result.tag;
    console.warn(
      `bynk: \`on rejected\` answered ${got} to a ${response.status} ${why.tag}; it must be a RawError of status ${response.status}, so the default response was sent`,
    );
    return response;
  }
  const headers = new Headers(response.headers);
  headers.delete("content-length");
  headers.set("content-type", result.contentType);
  return new Response(result.body as BodyInit, { status: result.status, headers });
}

// The `BoundaryError.kind`s the router emits when it refuses input *before* the
// handler — the only shapes that make an outcome `Rejected`. Kept in sync with
// the router's rejection bodies in `workers_entry.rs`.
const BOUNDARY_REJECTION_KINDS = new Set(["RefinementViolation", "MalformedJson", "MalformedForm", "StructuralMismatch"]);

// Whether `request`'s `Content-Type` is the media type a `@raw` route declares:
// parameters (`; charset=…`) are not part of the match, the comparison is
// case-insensitive, `"*/*"` accepts anything, and a missing header matches only
// that wildcard.
export function mediaTypeMatches(request: Request, declared: string): boolean {
  if (declared === "*/*") return true;
  const given = request.headers.get("content-type");
  if (given === null) return false;
  return given.split(";")[0].trim().toLowerCase() === declared.toLowerCase();
}

// #2341 (decision A): an HTTP handler's `headers: RequestHeaders` — a read-only
// view of the request's headers. `get` is case-insensitive and joins repeated
// headers with ", " (both the platform `Headers.get` contract); `bearer` is the
// token of an `Authorization: Bearer <token>` header and nothing else;
// `clientIp` is the address Cloudflare reports (`CF-Connecting-IP`, which
// `wrangler dev` sets too).
export interface RequestHeaders {
  get(name: string): Option<string>;
  bearer(): Option<string>;
  clientIp(): Option<string>;
}

/** #2341 (L4): the request's headers, or — in a test — `"name: value"` lines
 *  (`RequestHeaders.of`); a line without a colon is ignored. */
export function requestHeaders(source: Request | readonly string[]): RequestHeaders {
  let headers: Headers;
  if (source instanceof Request) {
    headers = source.headers;
  } else {
    headers = new Headers();
    for (const line of source) {
      const i = line.indexOf(":");
      if (i > 0) headers.append(line.slice(0, i).trim(), line.slice(i + 1).trim());
    }
  }
  const opt = (v: string | null): Option<string> =>
    v === null ? { tag: "None" } : { tag: "Some", value: v };
  const view: RequestHeaders = {
    get: (name: string) => opt(headers.get(name)),
    bearer: () => {
      const m = /^Bearer +([^\s]+) *$/i.exec(headers.get("authorization") ?? "");
      return m === null ? { tag: "None" } : { tag: "Some", value: m[1] };
    },
    clientIp: () => opt(headers.get("cf-connecting-ip")),
  };
  // The system-tier driver sends a test's headers on its real request.
  Object.defineProperty(view, REQUEST_HEADER_PAIRS, { value: headers, enumerable: false });
  return view;
}

const REQUEST_HEADER_PAIRS = Symbol("bynk.requestHeaderPairs");

/** #2341 (L4): a `RequestHeaders`' own headers as a plain record — what a
 *  system-tier driver spreads into the request it sends. */
export function requestHeaderPairs(view: RequestHeaders, dropAuthorization = false): Record<string, string> {
  const headers = (view as unknown as Record<symbol, Headers | undefined>)[REQUEST_HEADER_PAIRS];
  const out: Record<string, string> = {};
  headers?.forEach((v, k) => {
    out[k] = v;
  });
  // #2910: a case's extra headers never carry the credential; `by <Actor>` owns it.
  if (dropAuthorization) delete out["authorization"];
  return out;
}

// #2854 (decision B): an HTTP handler's `query: RequestQuery` — a read-only view
// of the request's query string. `get` is the first value of a key, `all` every
// value in order, `params` every key with its values. Names and values are
// percent-decoded, and `+` is a space, as `bynk.url.decodeQuery` does.
export interface RequestQuery {
  get(name: string): Option<string>;
  all(name: string): readonly string[];
  params(): ReadonlyMap<string, readonly string[]>;
}

const REQUEST_QUERY_SEARCH = Symbol("bynk.requestQuerySearch");

/** #2854: the request's query string, or — in a test — `"name=value"` pairs
 *  (`RequestQuery.of`). */
export function requestQuery(source: Request | readonly string[]): RequestQuery {
  const search =
    source instanceof Request
      ? new URL(source.url).searchParams
      : new URLSearchParams(source.join("&"));
  const view: RequestQuery = {
    get: (name: string) => {
      const v = search.get(name);
      return v === null ? { tag: "None" } : { tag: "Some", value: v };
    },
    all: (name: string) => search.getAll(name),
    params: () => {
      const out = new Map<string, readonly string[]>();
      search.forEach((_value, k) => {
        if (!out.has(k)) out.set(k, search.getAll(k));
      });
      return out;
    },
  };
  // The system-tier driver sends a test's query on its real request.
  Object.defineProperty(view, REQUEST_QUERY_SEARCH, { value: search, enumerable: false });
  return view;
}

/** #2854: a `RequestQuery`'s own query string (`?a=1&b=2`, or `""`) — what a
 *  system-tier driver appends to the path it requests. */
export function requestQuerySearch(view: RequestQuery): string {
  const search = (view as unknown as Record<symbol, URLSearchParams | undefined>)[
    REQUEST_QUERY_SEARCH
  ];
  const text = search?.toString() ?? "";
  return text === "" ? "" : `?${text}`;
}

/** #2854: a typed `query` record's wire form (what the record's `serialise_*`
 *  makes) as the query string a request for it carries: `?a=1&b=x&b=y`, or `""`
 *  when every field is absent. A list repeats its key, an `Option` is its value
 *  or nothing, and an enum is its variant's name. Test drivers use it. */
export function queryString(wire: JsonValue): string {
  const params = new URLSearchParams();
  const leaf = (v: JsonValue): string | undefined => {
    if (v === null) return undefined;
    if (typeof v === "object" && !Array.isArray(v)) {
      if (v.kind === "None") return undefined;
      if (v.kind === "Some") return leaf(v.value ?? null);
      return typeof v.kind === "string" ? v.kind : undefined;
    }
    return String(v);
  };
  if (typeof wire === "object" && wire !== null && !Array.isArray(wire)) {
    for (const [name, value] of Object.entries(wire)) {
      const items = Array.isArray(value) ? value : [value];
      for (const item of items) {
        const text = leaf(item);
        if (text !== undefined) params.append(name, text);
      }
    }
  }
  const text = params.toString();
  return text === "" ? "" : `?${text}`;
}

/** #3276: a typed `@multipart` body's wire form (what the record's `serialise_*`
 *  makes, a `File` being `{ filename, contentType, bytes: base64 }`) as the
 *  `FormData` a request for it carries. Test drivers use it; `fields` says which
 *  record fields are files. */
export function multipartForm(wire: JsonValue, fields: readonly QueryField[]): FormData {
  const form = new FormData();
  if (typeof wire !== "object" || wire === null || Array.isArray(wire)) return form;
  const some = (v: JsonValue): JsonValue | undefined => {
    if (typeof v === "object" && v !== null && !Array.isArray(v)) {
      if (v.kind === "None") return undefined;
      if (v.kind === "Some") return v.value ?? undefined;
    }
    return v;
  };
  for (const f of fields) {
    const value = wire[f.name];
    if (value === undefined || value === null) continue;
    for (const item of f.list && Array.isArray(value) ? value : [value]) {
      const v = some(item);
      if (v === undefined) continue;
      if (f.kind === "file") {
        const file = v as { filename: JsonValue; contentType: string; bytes: string };
        const name = some(file.filename);
        const raw = globalThis.atob(file.bytes);
        const bytes = Uint8Array.from(raw, (c) => c.charCodeAt(0));
        form.append(f.name, new Blob([bytes], { type: file.contentType }), typeof name === "string" ? name : "");
      } else if (v !== null && typeof v === "object" && !Array.isArray(v)) {
        if (typeof v.kind === "string") form.append(f.name, v.kind);
      } else {
        form.append(f.name, String(v));
      }
    }
  }
  return form;
}

/** #2854: how one field of a handler's typed `query` record reads off the wire. */
export interface QueryField {
  readonly name: string;
  /** The field's leaf wire form: `Int` and `Bool` are parsed, an `enum` is a
   *  unit variant named by the value, a `Decimal` and everything else is the text
   *  itself (`"decimal"` differs from `"string"` only in that a form's empty value
   *  is absent for it). A `"file"` is a `@multipart` file part. */
  readonly kind: "string" | "decimal" | "int" | "bool" | "enum" | "file";
  readonly list: boolean;
  /** An `Option` field: a present value is wrapped in `Some`, so a value that
   *  reads like the envelope (`?kind=None`) is still the leaf's, not `None`. */
  readonly option: boolean;
}

/** #2854 (decision A): decode the request's query string into the JSON object a
 *  record's codec reads, field by field. Keys the record does not declare are
 *  ignored. A key repeated on a field that is not a `List` is refused; a value
 *  that does not read as its field's type is passed on as text, so the record's
 *  codec reports it as a `StructuralMismatch` at `query.<field>`. The codec then
 *  checks refinements and fills defaults, as for a `body`. */
export function queryObject(
  request: Request,
  fields: readonly QueryField[],
): { ok: true; value: JsonValue } | { ok: false; error: BoundaryError } {
  return fieldsObject(new URL(request.url).searchParams, fields, "query", false);
}

/** #3277 (decision B): the `name=value` pairs of a `Cookie` header, in order
 *  (RFC 6265 §5.4): pairs split on `;`, each on its first `=`, names and values
 *  trimmed, one pair of double quotes dropped from a value. A pair with no `=` or
 *  an empty name is skipped. Values are **not** percent-decoded: a cookie value is
 *  opaque, and the writer emits what it was given. */
export function cookiePairs(header: string | null): [string, string][] {
  const out: [string, string][] = [];
  if (header === null) return out;
  for (const part of header.split(";")) {
    const eq = part.indexOf("=");
    if (eq < 0) continue;
    const name = part.slice(0, eq).trim();
    if (name === "") continue;
    let value = part.slice(eq + 1).trim();
    if (value.length >= 2 && value.startsWith('"') && value.endsWith('"')) value = value.slice(1, -1);
    out.push([name, value]);
  }
  return out;
}

/** #3277 (decisions A, B): decode the request's `Cookie` header into the JSON
 *  object a record's codec reads, field by field, by the typed `query` rules (a
 *  name sent twice fills a `List`, and is otherwise refused; an unknown cookie is
 *  ignored). A rejection names `cookies.<field>`. */
export function cookieObject(
  request: Request,
  fields: readonly QueryField[],
): { ok: true; value: JsonValue } | { ok: false; error: BoundaryError } {
  const search = new URLSearchParams();
  for (const [name, value] of cookiePairs(request.headers.get("cookie"))) search.append(name, value);
  return fieldsObject(search, fields, "cookies", false);
}

/** #3277: a `cookies` record's wire form as the `Cookie` header a request for it
 *  carries (`a=1; b=x; b=y`), `""` when every field is absent. A list repeats its
 *  name, an `Option` is its value or nothing. Test drivers use it. */
export function cookieString(wire: JsonValue): string {
  const query = queryString(wire);
  if (query === "") return "";
  // `queryString` form-encodes; a cookie value is not decoded, so it is written raw.
  const pairs: string[] = [];
  new URLSearchParams(query.slice(1)).forEach((value, name) => pairs.push(`${name}=${value}`));
  return pairs.join("; ");
}

/** #3276 (decisions B-D): decode a form body (`application/x-www-form-urlencoded`)
 *  into the JSON object a record's codec reads. The same reader as
 *  [`queryObject`] — one table of leaf rules — with the two differences a
 *  browser's form needs: a `Bool` also reads `on`/`off` (a checkbox's default
 *  value), and an empty value (`name=`, what an unfilled input sends) is absent
 *  for an `Option` and for every leaf but a plain `String`, which keeps `""` so a
 *  `NonEmpty` refinement still fires. Errors name `body.<field>`. */
export function formObject(
  text: string,
  fields: readonly QueryField[],
): { ok: true; value: JsonValue } | { ok: false; error: BoundaryError } {
  return fieldsObject(new URLSearchParams(text), fields, "body", true);
}

function fieldsObject(
  search: URLSearchParams,
  fields: readonly QueryField[],
  prefix: string,
  form: boolean,
): { ok: true; value: JsonValue } | { ok: false; error: BoundaryError } {
  const out: Record<string, JsonValue> = {};
  const leaf = (kind: QueryField["kind"], raw: string): JsonValue => {
    switch (kind) {
      case "int":
        return /^-?\d+$/.test(raw) && Number.isSafeInteger(Number(raw)) ? Number(raw) : raw;
      case "bool":
        if (form) {
          if (raw === "true" || raw === "on") return true;
          if (raw === "false" || raw === "off") return false;
          return raw;
        }
        return raw === "true" ? true : raw === "false" ? false : raw;
      case "enum":
        return { kind: raw };
      default:
        return raw;
    }
  };
  for (const f of fields) {
    // A browser sends an unfilled input as `name=`: absent, except for a plain
    // `String`, whose `""` the field's own refinements judge.
    const skipEmpty = form && (f.option || f.kind !== "string");
    const values = search.getAll(f.name).filter((v) => !(skipEmpty && v === ""));
    if (values.length === 0) continue;
    if (f.list) {
      out[f.name] = values.map((v) => leaf(f.kind, v));
    } else if (values.length > 1) {
      return {
        ok: false,
        error: {
          kind: "StructuralMismatch",
          path: `${prefix}.${f.name}`,
          expected: "a single value",
          actual: `${values.length} values`,
        },
      };
    } else {
      const v = leaf(f.kind, values[0]);
      out[f.name] = f.option ? { kind: "Some", value: v } : v;
    }
  }
  return { ok: true, value: out };
}

/** #3276: whether `request`'s `Content-Type` is `multipart/form-data` (parameters
 *  stripped, case-insensitive). The boundary parameter's presence is the parser's
 *  to judge: a body with none is a `MalformedForm`, not a `415`. */
export function multipartMediaTypeMatches(request: Request): boolean {
  const given = request.headers.get("content-type");
  return given !== null && given.split(";")[0].trim().toLowerCase() === "multipart/form-data";
}

/** What reading a multipart body gives: the JSON object the record's codec reads
 *  (a file field holds an empty placeholder, so the codec sees its shape without
 *  the file's bytes being copied into base64), and the real `File` values, which
 *  the caller lays over the decoded record. */
export type MultipartRead =
  | { ok: true; value: JsonValue; files: Record<string, unknown> }
  | { ok: false; status: 400 | 413; error: Record<string, JsonValue> };

/** The per-request caps a `@multipart` route enforces. */
export interface MultipartLimits {
  readonly maxFile: number | null;
  readonly maxParts: number;
}

const FILE_PLACEHOLDER: JsonValue = { filename: { kind: "None" }, contentType: "", bytes: "" };

/** #3276 (decisions B-F): read a `multipart/form-data` body, already held whole
 *  under the route's cap, into the record's JSON form plus its file parts. Text
 *  parts follow the form rules ([`formObject`]); a part with a filename is a file,
 *  and belongs to a `File` field. Too many parts, or a file over `maxFile`, is the
 *  `413`; a body that is not multipart at all (no boundary, no closing one) is a
 *  `MalformedForm`. */
export async function multipartObject(
  body: Uint8Array,
  contentType: string,
  fields: readonly QueryField[],
  limits: MultipartLimits,
): Promise<MultipartRead> {
  const tooLarge = (details: string): MultipartRead => ({
    ok: false,
    status: 413,
    error: { kind: "PayloadTooLarge", details },
  });
  let form: FormData;
  try {
    form = await new Request("https://multipart.invalid/", {
      method: "POST",
      headers: { "content-type": contentType },
      body: body as BodyInit,
    }).formData();
  } catch {
    return { ok: false, status: 400, error: { kind: "MalformedForm", details: "Invalid multipart body" } };
  }
  let parts = 0;
  const search = new URLSearchParams();
  const uploaded = new Map<string, File[]>();
  // `forEach`, not `entries()`: the emitted project's `lib` has no `DOM.Iterable`.
  const entries: [string, FormDataEntryValue][] = [];
  form.forEach((value, name) => entries.push([name, value]));
  for (const [name, value] of entries) {
    if (++parts > limits.maxParts) {
      return tooLarge(`more than ${limits.maxParts} parts`);
    }
    if (typeof value === "string") {
      search.append(name, value);
      continue;
    }
    if (limits.maxFile !== null && value.size > limits.maxFile) {
      return tooLarge(`a file part is over ${limits.maxFile} bytes`);
    }
    // A browser sends an unchosen file input as a part with `filename=""` and no
    // bytes. Node reads it as an empty `File`; workerd reads it as an empty string,
    // handled with the text parts below. Either way it is no file.
    if (value.name === "" && value.size === 0) continue;
    const list = uploaded.get(name);
    if (list === undefined) uploaded.set(name, [value]);
    else list.push(value);
  }
  const mismatch = (name: string, expected: string, actual: string): MultipartRead => ({
    ok: false,
    status: 400,
    error: { kind: "StructuralMismatch", path: `body.${name}`, expected, actual },
  });
  const textFields = fields.filter((f) => f.kind !== "file");
  const fileNames = new Set(fields.filter((f) => f.kind === "file").map((f) => f.name));
  // A file where a text field is declared (or a text value where a file is).
  for (const name of uploaded.keys()) {
    if (!fileNames.has(name) && textFields.some((f) => f.name === name)) {
      return mismatch(name, "text", "a file");
    }
  }
  for (const name of fileNames) {
    if (search.getAll(name).some((v) => v !== "")) return mismatch(name, "a file", "text");
  }
  const text = fieldsObject(search, textFields, "body", true);
  if (!text.ok) return { ok: false, status: 400, error: text.error as unknown as Record<string, JsonValue> };
  const value = text.value as Record<string, JsonValue>;
  const files: Record<string, unknown> = {};
  for (const f of fields) {
    if (f.kind !== "file") continue;
    const got = uploaded.get(f.name) ?? [];
    if (!f.list && got.length > 1) return mismatch(f.name, "a single file", `${got.length} files`);
    const read = await Promise.all(
      got.map(async (file) => ({
        filename: file.name === "" ? { tag: "None" } : { tag: "Some", value: file.name },
        contentType: file.type === "" ? "application/octet-stream" : file.type,
        bytes: new Uint8Array(await file.arrayBuffer()),
      })),
    );
    if (f.list) {
      value[f.name] = read.map(() => FILE_PLACEHOLDER);
      files[f.name] = read;
    } else if (read.length === 1) {
      value[f.name] = f.option ? { kind: "Some", value: FILE_PLACEHOLDER } : FILE_PLACEHOLDER;
      files[f.name] = f.option ? { tag: "Some", value: read[0] } : read[0];
    } else if (f.option) {
      files[f.name] = { tag: "None" };
    }
  }
  return { ok: true, value, files };
}

/** #3276 (decision H): whether `request`'s `Content-Type` is a form's. The media
 *  type must be `application/x-www-form-urlencoded` (parameters stripped, case
 *  insensitive) and a `charset` parameter, when present, must be UTF-8: the
 *  decoder reads UTF-8 and nothing else, so another charset is a `415` rather
 *  than silently mis-decoded text. */
export function formMediaTypeMatches(request: Request): boolean {
  const given = request.headers.get("content-type");
  if (given === null) return false;
  const [media, ...params] = given.split(";");
  if (media.trim().toLowerCase() !== "application/x-www-form-urlencoded") return false;
  for (const p of params) {
    const eq = p.indexOf("=");
    if (eq < 0 || p.slice(0, eq).trim().toLowerCase() !== "charset") continue;
    const charset = p.slice(eq + 1).trim().replace(/^"(.*)"$/, "$1").toLowerCase();
    if (charset !== "utf-8" && charset !== "utf8") return false;
  }
  return true;
}

/** #3276: a record's wire form (what the record's `serialise_*` makes) as a
 *  form body — `a=1&b=x&b=y`, `""` when every field is absent. A list repeats its
 *  key, an `Option` is its value or nothing, and an enum is its variant's name.
 *  `Form.encode` and the test drivers use it; `queryString` is the same text
 *  behind a `?`. */
export function formString(wire: JsonValue): string {
  const text = queryString(wire);
  return text === "" ? "" : text.slice(1);
}

export async function responseToHttpOutcome<T>(
  response: Response,
  deserialiseValue: (json: JsonValue) => Result<T, BoundaryError>,
): Promise<HttpOutcome<T>> {
  // Classify on the body's *shape*, not its status. A pre-handler rejection is a
  // `400` whose body is a `BoundaryError` carrying a recognised `kind`. A
  // handler that *ran* and returned `BadRequest(msg)` also yields a `400`, but
  // its body is `{ error: msg }` with no `kind` — the handler produced it, so it
  // is `Handled`, not `Rejected`. Reading `kind` (rather than the status) is what
  // keeps "the handler never ran" — the whole meaning of `Rejected` — accurate.
  if (response.status === 400) {
    let body: { kind?: unknown } | null = null;
    try {
      body = (await response.clone().json()) as { kind?: unknown };
    } catch {
      body = null;
    }
    if (body && typeof body.kind === "string" && BOUNDARY_REJECTION_KINDS.has(body.kind)) {
      const fields = body as Record<string, unknown>;
      return {
        tag: "Rejected",
        value: { ...fields, tag: body.kind, detail: rejectionDetail(fields) },
      };
    }
    // No boundary `kind` → the handler produced this `400`; fall through.
  }
  // A capped route's `413` and a raw route's `415` are the router's own
  // synthesised refusals, told from a handler's `PayloadTooLarge`/
  // `UnsupportedMediaType` (whose body is `{ error }`) by the `kind` they carry.
  if (response.status === 413 || response.status === 415) {
    let body: { kind?: unknown } | null = null;
    try {
      body = (await response.clone().json()) as { kind?: unknown };
    } catch {
      body = null;
    }
    if (body && (body.kind === "PayloadTooLarge" || body.kind === "UnsupportedMediaType")) {
      const fields = body as Record<string, unknown>;
      return {
        tag: "Rejected",
        value: { ...fields, tag: body.kind, detail: rejectionDetail(fields) },
      };
    }
  }
  // #707: a `405` is `Rejected(MethodNotAllowed)` only when it is the router's
  // *method fall-through* — a live path reached under a method it declares no
  // handler for. Classify on shape, not status: the router synthesises that 405
  // with an `Allow` header (`workers_entry.rs`), whereas a handler that ran and
  // returned `HttpResult.MethodNotAllowed` yields a bodyless 405 with no `Allow`
  // — the handler produced it, so it stays `Handled`. Same shape-vs-status care
  // as the `400` (`kind` body) path.
  if (response.status === 405 && response.headers.has("allow")) {
    return { tag: "Rejected", value: { tag: "MethodNotAllowed", detail: "" } };
  }
  const handled = await responseToHttpResult(response, deserialiseValue);
  return { tag: "Handled", value: handled };
}

// #706: the outcome of driving a secured route with *no credential* (`by
// Nobody`). A `401` here is the auth seam refusing the request before the
// handler ran — `Rejected(Unauthorized)` — which is unambiguous *because the
// caller sent no credential* (unlike the shared `responseToHttpOutcome`, where a
// `401` could be a handler-returned `Unauthorized`, so it stays `Handled`). Any
// other status is decoded normally: a route that is not actually secured lets
// the request through, and the case observes `Handled` rather than a false
// `Rejected`.
export async function responseToUnauthOutcome<T>(
  response: Response,
  deserialiseValue: (json: JsonValue) => Result<T, BoundaryError>,
): Promise<HttpOutcome<T>> {
  if (response.status === 401) {
    return { tag: "Rejected", value: { tag: "Unauthorized", detail: "" } };
  }
  return responseToHttpOutcome(response, deserialiseValue);
}

// v0.131 (ADR 0159): the cross-origin (CORS) policy a `from http` service carries
// via its `cors { }` section. The compiler synthesises one of these per
// CORS-enabled service and threads it into the entry router. `allowMethods` is
// derived from the service's routes at emit time (never restated by the author);
// `allowHeaders` carries the resolved list (the `content-type`/`Authorization`
// default, or the author's override). `maxAgeSecs` is the `Access-Control-Max-Age`
// in whole seconds, absent when the author gave no `maxAge`.
export interface CorsPolicy {
  readonly origins: readonly string[];
  readonly allowMethods: readonly string[];
  readonly allowHeaders: readonly string[];
  readonly credentials: boolean;
  readonly maxAgeSecs: number | null;
}

// Resolve the `Access-Control-Allow-Origin` value for a request, given the
// policy and the request's `Origin` header. A wildcard policy (`["*"]`) answers
// every origin with `*` (and needs no `Vary`, since the response does not depend
// on the request origin). A concrete allowlist **reflects** the request's origin
// when it matches — never echoes an unvalidated value — and returns `null` on no
// match, so the caller omits the header and the browser fails the request closed.
function corsResolveOrigin(policy: CorsPolicy, requestOrigin: string | null): string | null {
  if (policy.origins.length === 1 && policy.origins[0] === "*") return "*";
  if (requestOrigin !== null && policy.origins.includes(requestOrigin)) return requestOrigin;
  return null;
}

// Stamp the CORS response headers onto an already-built `Response`, in place.
// `Response.headers` is mutable, so this works uniformly across every
// `httpResultToResponse` shape — JSON, `Raw` bytes, a redirect, an error body,
// or an SSE `ReadableStream` — without reconstructing the response or touching
// its body. When the origin does not match the allowlist, no ACAO is added (the
// browser blocks the read); a reflected origin also gets `Vary: Origin` so a
// shared cache never serves one origin's grant to another.
export function applyCors<R extends Response>(
  response: R,
  policy: CorsPolicy,
  requestOrigin: string | null,
): R {
  const allowOrigin = corsResolveOrigin(policy, requestOrigin);
  if (allowOrigin === null) return response;
  response.headers.set("access-control-allow-origin", allowOrigin);
  if (allowOrigin !== "*") response.headers.append("vary", "Origin");
  if (policy.credentials) response.headers.set("access-control-allow-credentials", "true");
  return response;
}

// Build the `204 No Content` preflight response for an `OPTIONS` request against
// a CORS-enabled route. Answered by the entry router *before* the auth seam — a
// preflight is credential-less by spec, so it must not be rejected by a `by`
// actor / Bearer check. A non-allowlisted origin still gets a bodyless `204`, but
// without the `Access-Control-*` grant, so the browser's preflight check fails
// closed.
export function corsPreflightResponse(policy: CorsPolicy, requestOrigin: string | null): Response {
  const headers = new Headers();
  const allowOrigin = corsResolveOrigin(policy, requestOrigin);
  if (allowOrigin !== null) {
    headers.set("access-control-allow-origin", allowOrigin);
    if (allowOrigin !== "*") headers.append("vary", "Origin");
    if (policy.credentials) headers.set("access-control-allow-credentials", "true");
    headers.set("access-control-allow-methods", policy.allowMethods.join(", "));
    headers.set("access-control-allow-headers", policy.allowHeaders.join(", "));
    if (policy.maxAgeSecs !== null) {
      headers.set("access-control-max-age", String(policy.maxAgeSecs));
    }
  }
  return new Response(null, { status: 204, headers });
}

// v0.141 (ADR 0164): the security-headers policy a `from http` service carries
// via its `security { }` section. Unlike CORS, the compiler synthesises one of
// these for *every* `from http` service (not only those with a block), because
// the safe header is on by default — a service with no `security { }` still gets
// `{ nosniff: true, hstsMaxAgeSecs: null }`. `nosniff` stamps
// `X-Content-Type-Options: nosniff` (a JSON/text body a browser must not
// MIME-sniff into HTML); `hstsMaxAgeSecs`, when non-null, stamps
// `Strict-Transport-Security: max-age=…` (the opt-in HTTPS pin).
export interface SecurityPolicy {
  readonly nosniff: boolean;
  readonly hstsMaxAgeSecs: number | null;
}

// Stamp the security response headers onto an already-built `Response`, in place
// — the `applyCors` shape. `Response.headers` is mutable, so this works
// uniformly across every response family (JSON, `Raw` bytes, a redirect, an
// error body, an SSE stream, and the synthesised preflight / `405` / `304`).
// Composes with `applyCors`: the two set **disjoint** headers, so stamping order
// is irrelevant. `set` (not `append`) is idempotent and last-writer-wins, so a
// header also set at the edge (e.g. Cloudflare) is simply overwritten with the
// policy's value.
export function applySecurityHeaders<R extends Response>(
  response: R,
  policy: SecurityPolicy,
): R {
  if (policy.nosniff) response.headers.set("x-content-type-options", "nosniff");
  if (policy.hstsMaxAgeSecs !== null) {
    response.headers.set("strict-transport-security", `max-age=${policy.hstsMaxAgeSecs}`);
  }
  return response;
}

// v0.140 (ADR 0163): a **weak** validator over a response's serialised body, from
// a fast non-cryptographic hash (FNV-1a, 32-bit). Weak (`W/"…"`) is correct here —
// the guarantee is *semantic* equivalence of the representation, which weak
// validators exist for, and Bynk serves no byte-range requests (the one place a
// strong validator is required). Synchronous, so `httpResultToResponse` stays sync
// (no `await crypto.subtle`); a stronger hash (SHA-256) is a drop-in swap behind
// this name if ever needed.
export function weakETag(body: string): string {
  // FNV-1a over the string's UTF-16 code units. The exact byte basis is immaterial
  // to correctness — a validator only has to *change when the body changes* — so
  // hashing code units directly avoids a `TextEncoder` allocation on the hot path.
  let hash = 0x811c9dc5;
  for (let i = 0; i < body.length; i++) {
    hash ^= body.charCodeAt(i);
    // `* 16777619` (the FNV prime) in 32-bit via `Math.imul`.
    hash = Math.imul(hash, 0x01000193);
  }
  // `>>> 0` to an unsigned 32-bit; base-36 for a compact opaque token.
  return `W/"${(hash >>> 0).toString(36)}"`;
}

// Caching raw responses: the largest `Raw` body that gets the automatic weak
// `ETag` — 1 MiB. Hashing is synchronous and proportional to the body, so above
// this a `GET` carries no automatic validator; the handler supplies one through
// `RawTagged` instead.
export const RAW_WEAK_ETAG_MAX_BYTES = 1024 * 1024;

// Caching raw responses: `weakETag`'s FNV-1a, over a `Raw` body's octets, in the
// same `W/"<base36>"` form.
function weakETagBytes(body: Uint8Array): string {
  let hash = 0x811c9dc5;
  for (let i = 0; i < body.length; i++) {
    hash ^= body[i];
    hash = Math.imul(hash, 0x01000193);
  }
  return `W/"${(hash >>> 0).toString(36)}"`;
}

// Caching raw responses: an entity tag as RFC 9110 §8.8.3 writes it — an optional
// `W/` and a double-quoted run of `etagc` (`%x21 / %x23-7E`). The grammar's
// `obs-text` is left out, so every accepted tag is also a valid header value.
function isEntityTag(tag: string): boolean {
  return /^(W\/)?"[\x21\x23-\x7E]*"$/.test(tag);
}

// v0.140 (ADR 0163): stamp a `Cache-Control` freshness directive onto an
// already-built `Response`, in place — the opt-in half of caching, driven by a
// handler's `@cache(maxAge:, scope:)` annotation. `scope` is the author's
// `public`/`private` (default `private`, so a *shared* cache never stores unless
// the author opts in); `maxAgeSecs` is `maxAge` in whole seconds. Only called for
// a `GET` that carries `@cache`; a route without it emits no `Cache-Control` and
// stays revalidatable through its `ETag` alone. `immutable` (caching raw
// responses, RFC 8246) appends the directive: the author's promise that the
// representation at this URL never changes, so no cache revalidates it within
// `max-age`. The dispatch passes it only when `true`.
export function applyCache<R extends Response>(
  response: R,
  maxAgeSecs: number,
  scope: "public" | "private",
  immutable?: boolean,
): R {
  const directives = `${scope}, max-age=${maxAgeSecs}`;
  response.headers.set("cache-control", immutable ? `${directives}, immutable` : directives);
  return response;
}

// First-party registry L7 (#2414, Decision C): a Worker that declares an
// `EdgeCache` instance is served through Workers Cache, which stores what it may
// under RFC 9111, a `404` included (heuristically cacheable). So in such a
// Worker only a `@cache` route's `2xx` carries the route's freshness, and every
// other `GET`/`HEAD` response says `no-store`. `applyEdgeCache` is `applyCache`
// for a `2xx` and `no-store` otherwise; `noStore` marks a `GET` route without
// `@cache`; `edgeCacheGuard` runs last, on whatever the entry answered, and
// marks a `GET`/`HEAD` response that carries no `Cache-Control` yet (the
// router's `404`/`405`, a rejection, a fault). A `101` (a WebSocket upgrade,
// which the cache bypasses) passes through untouched.
export function applyEdgeCache<R extends Response>(
  response: R,
  maxAgeSecs: number,
  scope: "public" | "private",
  immutable?: boolean,
): R {
  if (response.status >= 200 && response.status < 300) {
    return applyCache(response, maxAgeSecs, scope, immutable);
  }
  return noStore(response);
}

// Review of #2417: a Worker that negotiates its locale from `Accept-Language`
// answers one URL differently per language, so an edge-cached response names
// the header in `Vary`, and the cache keeps one variant per language.
export function varyAcceptLanguage<R extends Response>(response: R): R {
  const vary = response.headers.get("vary");
  if (vary === null) {
    response.headers.set("vary", "accept-language");
  } else if (!vary.toLowerCase().split(",").some((v) => v.trim() === "accept-language" || v.trim() === "*")) {
    response.headers.set("vary", `${vary}, accept-language`);
  }
  return response;
}

export function noStore<R extends Response>(response: R): R {
  response.headers.set("cache-control", "no-store");
  return response;
}

export function edgeCacheGuard(request: Request, response: Response): Response {
  if (request.method !== "GET" && request.method !== "HEAD") return response;
  if (response.status === 101 || response.headers.has("cache-control")) return response;
  try {
    return noStore(response);
  } catch {
    // A response with immutable headers is copied, not mutated.
    const copy = new Response(response.body, response);
    copy.headers.set("cache-control", "no-store");
    return copy;
  }
}

// v0.140 (ADR 0163): the conditional-`GET` router decision. When the built
// response carries a weak `ETag` and the request's `If-None-Match` lists a
// matching validator, answer `304 Not Modified` with an empty body, copying the
// `ETag` and any `Cache-Control` across (RFC 9110 §15.4.5 — a `304` carries the
// caching headers a `200` would have). Otherwise the response passes through
// untouched. Like the CORS preflight and the method-semantics `405`, the `304` is
// a *router* response synthesised from the request validator, never an
// `HttpResult` variant (ADR 0126 D4). Runs *after* the handler (the body must
// exist to hash), so it saves bandwidth, not server work.
export function notModifiedIfMatch<R extends Response>(
  response: R,
  request: Request,
): R | Response {
  const etag = response.headers.get("etag");
  if (etag === null) return response;
  const ifNoneMatch = request.headers.get("if-none-match");
  if (ifNoneMatch === null) return response;
  if (!ifNoneMatchMatches(ifNoneMatch, etag)) return response;
  const headers = new Headers();
  headers.set("etag", etag);
  const cacheControl = response.headers.get("cache-control");
  if (cacheControl !== null) headers.set("cache-control", cacheControl);
  return new Response(null, { status: 304, headers });
}

// RFC 9110 §13.1.2: `If-None-Match` is a comma-separated list of entity tags (or a
// bare `*`, matching any current representation), compared with the **weak**
// comparison function — the `W/` prefix is ignored on both sides. Our validators
// are always weak and a conditional client echoes back exactly what we sent, but
// handling the list, `*`, and weak comparison keeps this correct against any
// conformant client.
//
// The list is tokenised by entity tag, not split on `,`: a handler-supplied
// `RawTagged` tag may itself contain a comma (`etagc` admits `%x2C`), so
// `"a,b"` must stay one tag.
function ifNoneMatchMatches(headerValue: string, etag: string): boolean {
  const target = stripWeakPrefix(etag);
  for (const [candidate] of headerValue.matchAll(/\*|(?:W\/)?"[^"]*"/g)) {
    if (candidate === "*") return true;
    if (stripWeakPrefix(candidate) === target) return true;
  }
  return false;
}

function stripWeakPrefix(tag: string): string {
  return tag.startsWith("W/") ? tag.slice(2) : tag;
}

// #1653: the `__` aliases emitted code imports.
export { applyCache as __applyCache, applyCors as __applyCors, applyEdgeCache as __applyEdgeCache, applySecurityHeaders as __applySecurityHeaders, corsPreflightResponse as __corsPreflightResponse, edgeCacheGuard as __edgeCacheGuard, headResponse as __headResponse, withPlainJson as __withPlainJson, httpResultToResponse as __httpResultToResponse, matchPath as __matchPath, mediaTypeMatches as __mediaTypeMatches, multipartForm as __multipartForm, multipartMediaTypeMatches as __multipartMediaTypeMatches, multipartObject as __multipartObject, requestHeaders as __requestHeaders, formMediaTypeMatches as __formMediaTypeMatches, formObject as __formObject, formString as __formString, cookieObject as __cookieObject, cookieString as __cookieString, queryObject as __queryObject, queryString as __queryString, requestQuery as __requestQuery, requestQuerySearch as __requestQuerySearch, renderRejection as __renderRejection, requestHeaderPairs as __requestHeaderPairs, noStore as __noStore, notModifiedIfMatch as __notModifiedIfMatch, responseToHttpOutcome as __responseToHttpOutcome, responseToHttpResult as __responseToHttpResult, responseToUnauthOutcome as __responseToUnauthOutcome, varyAcceptLanguage as __varyAcceptLanguage };
export type { CorsPolicy as __CorsPolicy, SecurityPolicy as __SecurityPolicy };
