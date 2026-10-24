import { test } from "node:test";
import assert from "node:assert/strict";
import {
  HttpResult,
  matchPath,
  httpResultToResponse,
  headResponse,
  applyCors,
  corsPreflightResponse,
  applySecurityHeaders,
  weakETag,
  applyCache,
  applyEdgeCache,
  edgeCacheGuard,
  noStore,
  notModifiedIfMatch,
  RAW_WEAK_ETAG_MAX_BYTES,
  responseToHttpOutcome,
  responseToUnauthOutcome,
  mediaTypeMatches,
  requestHeaders,
  requestHeaderPairs,
  renderRejection,
  responseToHttpResult,
  type CorsPolicy,
  type SecurityPolicy,
} from "../src/http.ts";
import type { JsonValue } from "../src/boundary.ts";

const listPolicy: CorsPolicy = {
  origins: ["https://app.example.com", "https://admin.example.com"],
  allowMethods: ["GET", "POST", "OPTIONS"],
  allowHeaders: ["content-type", "authorization"],
  credentials: true,
  maxAgeSecs: 3600,
};

const wildcardPolicy: CorsPolicy = {
  origins: ["*"],
  allowMethods: ["GET", "OPTIONS"],
  allowHeaders: ["content-type"],
  credentials: false,
  maxAgeSecs: null,
};

test("matchPath: captures and decodes params", () => {
  assert.deepEqual(matchPath("/orders/:id", "/orders/abc%20123"), { params: { id: "abc 123" } });
  assert.deepEqual(matchPath("/a/:x/b/:y", "/a/1/b/2"), { params: { x: "1", y: "2" } });
});

test("matchPath: returns null on literal or length mismatch", () => {
  assert.equal(matchPath("/orders/:id", "/customers/1"), null);
  assert.equal(matchPath("/orders/:id", "/orders/1/extra"), null);
});

const id = (v: number): JsonValue => v;

test("httpResultToResponse: status codes map per variant", async () => {
  // 2xx success.
  assert.equal(httpResultToResponse(HttpResult.Ok(1), id).status, 200);
  assert.equal(httpResultToResponse(HttpResult.Created(1), id).status, 201);
  assert.equal(httpResultToResponse(HttpResult.RawCreated(new Uint8Array(0), "text/plain"), id).status, 201);
  assert.equal(httpResultToResponse(HttpResult.Accepted(1), id).status, 202);
  assert.equal(httpResultToResponse(HttpResult.NoContent, id).status, 204);
  // 3xx redirection.
  assert.equal(httpResultToResponse(HttpResult.MovedPermanently("/x"), id).status, 301);
  assert.equal(httpResultToResponse(HttpResult.Found("/x"), id).status, 302);
  assert.equal(httpResultToResponse(HttpResult.SeeOther("/x"), id).status, 303);
  assert.equal(httpResultToResponse(HttpResult.TemporaryRedirect("/x"), id).status, 307);
  assert.equal(httpResultToResponse(HttpResult.PermanentRedirect("/x"), id).status, 308);
  // 4xx client error.
  assert.equal(httpResultToResponse(HttpResult.BadRequest("b"), id).status, 400);
  assert.equal(httpResultToResponse(HttpResult.Unauthorized, id).status, 401);
  assert.equal(httpResultToResponse(HttpResult.Forbidden, id).status, 403);
  assert.equal(httpResultToResponse(HttpResult.NotFound, id).status, 404);
  assert.equal(httpResultToResponse(HttpResult.MethodNotAllowed, id).status, 405);
  assert.equal(httpResultToResponse(HttpResult.NotAcceptable, id).status, 406);
  assert.equal(httpResultToResponse(HttpResult.RequestTimeout, id).status, 408);
  assert.equal(httpResultToResponse(HttpResult.Conflict("c"), id).status, 409);
  assert.equal(httpResultToResponse(HttpResult.Gone, id).status, 410);
  assert.equal(httpResultToResponse(HttpResult.LengthRequired, id).status, 411);
  assert.equal(httpResultToResponse(HttpResult.PayloadTooLarge("p"), id).status, 413);
  assert.equal(httpResultToResponse(HttpResult.UnsupportedMediaType("m"), id).status, 415);
  assert.equal(httpResultToResponse(HttpResult.UnprocessableEntity("u"), id).status, 422);
  assert.equal(httpResultToResponse(HttpResult.TooManyRequests("t"), id).status, 429);
  assert.equal(httpResultToResponse(HttpResult.UnavailableForLegalReasons("l"), id).status, 451);
  // 5xx server error.
  assert.equal(httpResultToResponse(HttpResult.ServerError("s"), id).status, 500);
  assert.equal(httpResultToResponse(HttpResult.NotImplemented("n"), id).status, 501);
  assert.equal(httpResultToResponse(HttpResult.BadGateway("g"), id).status, 502);
  assert.equal(httpResultToResponse(HttpResult.ServiceUnavailable("s"), id).status, 503);
  assert.equal(httpResultToResponse(HttpResult.GatewayTimeout("g"), id).status, 504);
});

test("httpResultToResponse: Streaming frames a Stream as SSE", async () => {
  async function* events() {
    yield "tick-1";
    yield "multi\nline";
  }
  const res = httpResultToResponse(HttpResult.Streaming(events()), id);
  assert.equal(res.status, 200);
  assert.equal(res.headers.get("content-type"), "text/event-stream");
  assert.equal(res.headers.get("cache-control"), "no-cache");
  // Each element is one SSE event; a multi-line element becomes multiple
  // `data:` lines, each event terminated by a blank line.
  assert.equal(await res.text(), "data: tick-1\n\ndata: multi\ndata: line\n\n");
});

test("httpResultToResponse: redirects carry a Location header and no body", async () => {
  const res = httpResultToResponse(HttpResult.Found("https://bynk.dev/target"), id);
  assert.equal(res.status, 302);
  assert.equal(res.headers.get("location"), "https://bynk.dev/target");
  assert.equal(await res.text(), "");
});

test("httpResultToResponse: TooManyRequests carries an { error } body", async () => {
  const res = httpResultToResponse(HttpResult.TooManyRequests("slow down"), id);
  assert.equal(res.status, 429);
  assert.deepEqual(await res.json(), { error: "slow down" });
});

test("httpResultToResponse: Ok carries the serialised value; NoContent is empty", async () => {
  const ok = httpResultToResponse(HttpResult.Ok(42), id);
  assert.equal(await ok.json(), 42);
  const empty = httpResultToResponse(HttpResult.NoContent, id);
  assert.equal(await empty.text(), "");
});

test("httpResultToResponse: error variants carry an { error } body", async () => {
  const res = httpResultToResponse(HttpResult.BadRequest("bad input"), id);
  assert.deepEqual(await res.json(), { error: "bad input" });
});

test("headResponse: preserves status and headers, empties the body", async () => {
  const res = headResponse(httpResultToResponse(HttpResult.Ok(1), id));
  assert.equal(res.status, 200);
  assert.equal(res.headers.get("content-type"), "application/json");
  assert.equal(await res.text(), "");
  assert.equal(res.body, null);
});

test("headResponse: does not drain a Streaming body", async () => {
  let completed = false;
  const stream = (async function* () {
    yield "first";
    yield "second";
    completed = true;
  })();
  const res = headResponse(httpResultToResponse(HttpResult.Streaming(stream), id));
  assert.equal(res.status, 200);
  assert.equal(res.headers.get("content-type"), "text/event-stream");
  assert.equal(res.body, null);
  // Give any eagerly-scheduled stream work a turn: the discarded SSE
  // ReadableStream is never consumed by a reader, so the generator never runs
  // to completion (it is not drained).
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(completed, false);
});

test("applyCors: reflects a matched origin and sets Vary + credentials", () => {
  const res = applyCors(new Response("ok"), listPolicy, "https://app.example.com");
  assert.equal(res.headers.get("access-control-allow-origin"), "https://app.example.com");
  assert.equal(res.headers.get("vary"), "Origin");
  assert.equal(res.headers.get("access-control-allow-credentials"), "true");
});

test("applyCors: omits ACAO for a non-allowlisted origin (fail closed)", () => {
  const res = applyCors(new Response("ok"), listPolicy, "https://evil.example.com");
  assert.equal(res.headers.get("access-control-allow-origin"), null);
  assert.equal(res.headers.get("vary"), null);
});

test("applyCors: wildcard answers any origin with * and no Vary", () => {
  const res = applyCors(new Response("ok"), wildcardPolicy, "https://anything.example.com");
  assert.equal(res.headers.get("access-control-allow-origin"), "*");
  assert.equal(res.headers.get("vary"), null);
  assert.equal(res.headers.get("access-control-allow-credentials"), null);
});

test("corsPreflightResponse: 204 with methods/headers/max-age for a matched origin", () => {
  const res = corsPreflightResponse(listPolicy, "https://admin.example.com");
  assert.equal(res.status, 204);
  assert.equal(res.headers.get("access-control-allow-origin"), "https://admin.example.com");
  assert.equal(res.headers.get("access-control-allow-methods"), "GET, POST, OPTIONS");
  assert.equal(res.headers.get("access-control-allow-headers"), "content-type, authorization");
  assert.equal(res.headers.get("access-control-max-age"), "3600");
  assert.equal(res.headers.get("access-control-allow-credentials"), "true");
});

test("corsPreflightResponse: 204 without a grant for a non-allowlisted origin", () => {
  const res = corsPreflightResponse(listPolicy, "https://evil.example.com");
  assert.equal(res.status, 204);
  assert.equal(res.headers.get("access-control-allow-origin"), null);
  assert.equal(res.headers.get("access-control-allow-methods"), null);
});

// v0.141 (ADR 0164): security response headers — nosniff on by default, HSTS opt-in.

const defaultSecurity: SecurityPolicy = { nosniff: true, hstsMaxAgeSecs: null };
const hstsSecurity: SecurityPolicy = { nosniff: true, hstsMaxAgeSecs: 15552000 };
const optedOutSecurity: SecurityPolicy = { nosniff: false, hstsMaxAgeSecs: null };

test("applySecurityHeaders: default stamps nosniff, no HSTS", () => {
  const res = applySecurityHeaders(new Response("ok"), defaultSecurity);
  assert.equal(res.headers.get("x-content-type-options"), "nosniff");
  assert.equal(res.headers.get("strict-transport-security"), null);
});

test("applySecurityHeaders: HSTS opt-in stamps max-age alongside nosniff", () => {
  const res = applySecurityHeaders(new Response("ok"), hstsSecurity);
  assert.equal(res.headers.get("x-content-type-options"), "nosniff");
  assert.equal(res.headers.get("strict-transport-security"), "max-age=15552000");
});

test("applySecurityHeaders: nosniff:false opts out of every security header", () => {
  const res = applySecurityHeaders(new Response("ok"), optedOutSecurity);
  assert.equal(res.headers.get("x-content-type-options"), null);
  assert.equal(res.headers.get("strict-transport-security"), null);
});

test("applySecurityHeaders: composes with applyCors (disjoint headers, order-independent)", () => {
  const res = applySecurityHeaders(
    applyCors(new Response("ok"), listPolicy, "https://app.example.com"),
    hstsSecurity,
  );
  // CORS headers survive the security stamp...
  assert.equal(res.headers.get("access-control-allow-origin"), "https://app.example.com");
  // ...and the security headers are present too.
  assert.equal(res.headers.get("x-content-type-options"), "nosniff");
  assert.equal(res.headers.get("strict-transport-security"), "max-age=15552000");
});

// v0.140 (ADR 0163): conditional caching — weak ETag, opt-in freshness, 304.

test("weakETag: stable, weak, and body-sensitive", () => {
  const a = weakETag('{"n":1}');
  assert.equal(a, weakETag('{"n":1}')); // deterministic
  assert.match(a, /^W\/"[0-9a-z]+"$/); // weak, quoted, compact
  assert.notEqual(a, weakETag('{"n":2}')); // changes with the body
});

test("httpResultToResponse: weakEtag opt-in stamps ETag only on Ok", async () => {
  const ok = httpResultToResponse(HttpResult.Ok(1), id, { weakEtag: true });
  assert.equal(ok.status, 200);
  assert.equal(ok.headers.get("etag"), weakETag("1"));
  assert.equal(await ok.text(), "1");

  // Created/Accepted are not eligible even with the flag; the body is unchanged.
  assert.equal(
    httpResultToResponse(HttpResult.Created(1), id, { weakEtag: true }).headers.get("etag"),
    null,
  );

  // Without the opt-in, an Ok is byte-identical to the pre-caching behaviour.
  assert.equal(httpResultToResponse(HttpResult.Ok(1), id).headers.get("etag"), null);
});

test("applyCache: sets Cache-Control from scope + maxAge, in place", () => {
  const res = applyCache(httpResultToResponse(HttpResult.Ok(1), id, { weakEtag: true }), 300, "private");
  assert.equal(res.headers.get("cache-control"), "private, max-age=300");
  assert.equal(
    applyCache(httpResultToResponse(HttpResult.Ok(1), id), 60, "public").headers.get("cache-control"),
    "public, max-age=60",
  );
});

test("notModifiedIfMatch: matching If-None-Match yields an empty 304 with ETag + Cache-Control", async () => {
  const built = applyCache(httpResultToResponse(HttpResult.Ok(1), id, { weakEtag: true }), 300, "private");
  const etag = built.headers.get("etag")!;
  const req = new Request("https://x/y", { headers: { "if-none-match": etag } });
  const res = notModifiedIfMatch(built, req);
  assert.equal(res.status, 304);
  assert.equal(res.headers.get("etag"), etag);
  assert.equal(res.headers.get("cache-control"), "private, max-age=300");
  assert.equal(await res.text(), ""); // empty body
});

test("notModifiedIfMatch: stale or absent If-None-Match passes the 200 through", async () => {
  const built = httpResultToResponse(HttpResult.Ok(1), id, { weakEtag: true });
  // Stale validator.
  const stale = notModifiedIfMatch(
    built,
    new Request("https://x/y", { headers: { "if-none-match": 'W/"deadbeef"' } }),
  );
  assert.equal(stale.status, 200);
  assert.equal(await stale.clone().text(), "1");
  // No If-None-Match at all.
  assert.equal(notModifiedIfMatch(built, new Request("https://x/y")).status, 200);
});

test("notModifiedIfMatch: handles a list and the wildcard, and never 304s a body with no ETag", () => {
  const built = httpResultToResponse(HttpResult.Ok(1), id, { weakEtag: true });
  const etag = built.headers.get("etag")!;
  // A comma-separated list containing the tag matches.
  const list = new Request("https://x/y", { headers: { "if-none-match": `W/"other", ${etag}` } });
  assert.equal(notModifiedIfMatch(built, list).status, 304);
  // `*` matches any current representation.
  const star = new Request("https://x/y", { headers: { "if-none-match": "*" } });
  assert.equal(notModifiedIfMatch(built, star).status, 304);
  // A response with no ETag (e.g. a Streaming GET, or a Raw body over 1 MiB) is never revalidated.
  const noEtag = httpResultToResponse(HttpResult.Ok(1), id);
  assert.equal(notModifiedIfMatch(noEtag, star).status, 200);
});

// Caching raw responses: a small `Raw` body gets the weak ETag on a GET, a large
// one none; `RawTagged` sends its own tag, validated; `immutable` is appended.

test("httpResultToResponse: a Raw GET body up to 1 MiB carries a weak ETag over its bytes", async () => {
  const bytes = new TextEncoder().encode("User-agent: *");
  const res = httpResultToResponse(HttpResult.Raw(bytes, "text/plain"), id, { weakEtag: true });
  const etag = res.headers.get("etag");
  assert.match(etag!, /^W\/"[0-9a-z]+"$/);
  assert.equal(res.headers.get("content-type"), "text/plain");
  assert.equal(await res.text(), "User-agent: *");
  // Stable over the same bytes, and body-sensitive.
  const again = httpResultToResponse(HttpResult.Raw(bytes, "text/plain"), id, { weakEtag: true });
  assert.equal(again.headers.get("etag"), etag);
  const other = new TextEncoder().encode("User-agent: x");
  assert.notEqual(
    httpResultToResponse(HttpResult.Raw(other, "text/plain"), id, { weakEtag: true }).headers.get("etag"),
    etag,
  );
  // Without the GET opt-in, no validator.
  assert.equal(httpResultToResponse(HttpResult.Raw(bytes, "text/plain"), id).headers.get("etag"), null);
  // And a matching If-None-Match revalidates it to 304.
  const req = new Request("https://x/y", { headers: { "if-none-match": etag! } });
  assert.equal(notModifiedIfMatch(res, req).status, 304);
});

test("httpResultToResponse: the Raw weak ETag stops at exactly 1 MiB", () => {
  assert.equal(RAW_WEAK_ETAG_MAX_BYTES, 1048576);
  const atLimit = new Uint8Array(RAW_WEAK_ETAG_MAX_BYTES);
  const over = new Uint8Array(RAW_WEAK_ETAG_MAX_BYTES + 1);
  const opts = { weakEtag: true };
  assert.notEqual(httpResultToResponse(HttpResult.Raw(atLimit, "application/octet-stream"), id, opts).headers.get("etag"), null);
  assert.equal(httpResultToResponse(HttpResult.Raw(over, "application/octet-stream"), id, opts).headers.get("etag"), null);
});

test("httpResultToResponse: RawTagged sends its own tag, unhashed, on any size", async () => {
  const big = new Uint8Array(RAW_WEAK_ETAG_MAX_BYTES + 1);
  const res = httpResultToResponse(HttpResult.RawTagged(big, "application/octet-stream", '"sha256-ab"'), id, {
    weakEtag: true,
  });
  assert.equal(res.status, 200);
  assert.equal(res.headers.get("etag"), '"sha256-ab"');
  assert.equal((await res.arrayBuffer()).byteLength, big.byteLength);
  // Weak tags are accepted, and the tag is sent without the GET opt-in too.
  const weak = httpResultToResponse(HttpResult.RawTagged(big, "text/plain", 'W/"7"'), id);
  assert.equal(weak.headers.get("etag"), 'W/"7"');
  // A strong tag revalidates (weak comparison) to 304.
  const req = new Request("https://x/y", { headers: { "if-none-match": 'W/"sha256-ab"' } });
  assert.equal(notModifiedIfMatch(res, req).status, 304);
});

test("notModifiedIfMatch: a RawTagged tag containing a comma still revalidates", () => {
  const res = httpResultToResponse(HttpResult.RawTagged(new Uint8Array(1), "text/plain", '"sha256:a,b"'), id);
  const echoed = new Request("https://x/y", { headers: { "if-none-match": '"sha256:a,b"' } });
  assert.equal(notModifiedIfMatch(res, echoed).status, 304);
  // In a list, and weakly compared.
  const list = new Request("https://x/y", { headers: { "if-none-match": 'W/"other", W/"sha256:a,b"' } });
  assert.equal(notModifiedIfMatch(res, list).status, 304);
  // Its halves are not tags of their own.
  const halves = new Request("https://x/y", { headers: { "if-none-match": '"sha256:a", "b"' } });
  assert.equal(notModifiedIfMatch(res, halves).status, 200);
});

test("httpResultToResponse: a malformed RawTagged tag throws rather than send a bad header", () => {
  const body = new Uint8Array(1);
  for (const bad of ["v1", '"a"b"', '"sp ace"', 'w/"x"', '"x', "", '"ü"']) {
    assert.throws(
      () => httpResultToResponse(HttpResult.RawTagged(body, "text/plain", bad), id, { weakEtag: true }),
      /RawTagged: .* is not an entity tag/,
      `tag ${JSON.stringify(bad)} must be refused`,
    );
  }
  // The empty quoted tag is legal (RFC 9110 §8.8.3 `*etagc`).
  assert.equal(httpResultToResponse(HttpResult.RawTagged(body, "text/plain", '""'), id).headers.get("etag"), '""');
});

test("applyCache: immutable appends the RFC 8246 directive", () => {
  const res = applyCache(httpResultToResponse(HttpResult.Ok(1), id), 31536000, "public", true);
  assert.equal(res.headers.get("cache-control"), "public, max-age=31536000, immutable");
  assert.equal(
    applyCache(httpResultToResponse(HttpResult.Ok(1), id), 60, "public", false).headers.get("cache-control"),
    "public, max-age=60",
  );
});

// L7 (#2414, Decision C): an opted-in Worker keeps the route's freshness on a
// `2xx` alone; anything else it answers to a `GET`/`HEAD` says `no-store`.
test("applyEdgeCache: a 2xx keeps the route's Cache-Control, anything else is no-store", () => {
  const ok = applyEdgeCache(httpResultToResponse(HttpResult.Ok(1), id), 31536000, "public", true);
  assert.equal(ok.headers.get("cache-control"), "public, max-age=31536000, immutable");
  const created = applyEdgeCache(httpResultToResponse(HttpResult.Created(1), id), 60, "public");
  assert.equal(created.headers.get("cache-control"), "public, max-age=60");
  const missing = applyEdgeCache(new Response("no", { status: 404 }), 31536000, "public", true);
  assert.equal(missing.headers.get("cache-control"), "no-store");
  const failed = applyEdgeCache(new Response("x", { status: 500 }), 60, "public");
  assert.equal(failed.headers.get("cache-control"), "no-store");
});

test("noStore: marks a response no-store, in place", () => {
  const res = new Response("x");
  assert.equal(noStore(res), res);
  assert.equal(res.headers.get("cache-control"), "no-store");
});

test("edgeCacheGuard: fills only a GET/HEAD response with no Cache-Control", () => {
  const get = new Request("https://x/y");
  const head = new Request("https://x/y", { method: "HEAD" });
  const post = new Request("https://x/y", { method: "POST" });
  assert.equal(edgeCacheGuard(get, new Response("Not Found", { status: 404 })).headers.get("cache-control"), "no-store");
  assert.equal(edgeCacheGuard(head, new Response(null, { status: 405 })).headers.get("cache-control"), "no-store");
  const cached = applyCache(new Response("x"), 60, "public");
  assert.equal(edgeCacheGuard(get, cached).headers.get("cache-control"), "public, max-age=60");
  assert.equal(edgeCacheGuard(post, new Response("x")).headers.get("cache-control"), null);
  // A response whose headers can't change is copied.
  const frozen = Response.redirect("https://x/z", 302);
  const guarded = edgeCacheGuard(get, frozen);
  assert.equal(guarded.status, 302);
  assert.equal(guarded.headers.get("cache-control"), "no-store");
});

// #1934: every `Rejected` carries a `detail` string, the router's description
// of a refused body or path, and `""` for the seam's and router's refusals.
test("responseToHttpOutcome gives each rejection a detail", async () => {
  const deser = (json: JsonValue) => ({ tag: "Ok" as const, value: json });
  const rejected = (body: unknown) =>
    responseToHttpOutcome(new Response(JSON.stringify(body), { status: 400 }), deser);
  const detailOf = async (p: Promise<{ tag: string; value: unknown }>) => {
    const o = await p;
    assert.equal(o.tag, "Rejected");
    return (o.value as { detail: string }).detail;
  };
  assert.equal(await detailOf(rejected({ kind: "MalformedJson", details: "Unexpected token" })), "Unexpected token");
  assert.equal(
    await detailOf(rejected({ kind: "StructuralMismatch", path: "$.sku", expected: "string", actual: "number" })),
    "$.sku: expected string, got number",
  );
  assert.equal(
    await detailOf(rejected({ kind: "RefinementViolation", path: "$.sku", violation: { field: "sku", message: "must not be empty", value: "" } })),
    "$.sku: must not be empty",
  );
  const notAllowed = new Response(null, { status: 405, headers: { allow: "POST" } });
  assert.equal(await detailOf(responseToHttpOutcome(notAllowed, deser)), "");
  assert.equal(await detailOf(responseToUnauthOutcome(new Response(null, { status: 401 }), deser)), "");
});

test("responseToHttpOutcome classifies the router's 413 and 415 by their kind", async () => {
  const deser = (json: JsonValue) => ({ tag: "Ok" as const, value: json });
  const router413 = new Response(JSON.stringify({ kind: "PayloadTooLarge", details: "request body exceeds 8 bytes" }), { status: 413 });
  const o413 = await responseToHttpOutcome(router413, deser);
  assert.deepEqual(
    { tag: o413.tag, rejection: (o413.value as { tag: string }).tag, detail: (o413.value as { detail: string }).detail },
    { tag: "Rejected", rejection: "PayloadTooLarge", detail: "request body exceeds 8 bytes" },
  );
  const router415 = new Response(JSON.stringify({ kind: "UnsupportedMediaType", details: "expected Content-Type image/png" }), { status: 415 });
  const o415 = await responseToHttpOutcome(router415, deser);
  assert.equal(o415.tag, "Rejected");
  assert.equal((o415.value as { tag: string }).tag, "UnsupportedMediaType");
  // A handler's own `PayloadTooLarge` has no `kind`: it ran, so it is `Handled`.
  const handled = await responseToHttpOutcome(httpResultToResponse(HttpResult.PayloadTooLarge("too big"), (v) => v), deser);
  assert.equal(handled.tag, "Handled");
});

test("mediaTypeMatches ignores parameters and case, and `*/*` takes anything", () => {
  const req = (contentType: string | null) =>
    new Request("https://x.test/", { method: "POST", headers: contentType === null ? {} : { "content-type": contentType } });
  assert.equal(mediaTypeMatches(req("image/png"), "image/png"), true);
  assert.equal(mediaTypeMatches(req("Image/PNG; charset=binary"), "image/png"), true);
  assert.equal(mediaTypeMatches(req("image/jpeg"), "image/png"), false);
  assert.equal(mediaTypeMatches(req("image/png+x"), "image/png"), false);
  assert.equal(mediaTypeMatches(req(null), "image/png"), false);
  assert.equal(mediaTypeMatches(req(null), "*/*"), true);
  assert.equal(mediaTypeMatches(req("text/plain"), "*/*"), true);
});

// #2341 (L4): a handler's headers view, from a request or from test lines.
test("requestHeaders: reads a request's headers case-insensitively", () => {
  const h = requestHeaders(
    new Request("https://x/", {
      headers: { "X-Name": "ann", authorization: "bearer  tok-1 ", "cf-connecting-ip": "10.0.0.1" },
    }),
  );
  assert.deepEqual(h.get("x-name"), { tag: "Some", value: "ann" });
  assert.deepEqual(h.get("missing"), { tag: "None" });
  assert.deepEqual(h.bearer(), { tag: "Some", value: "tok-1" });
  assert.deepEqual(h.clientIp(), { tag: "Some", value: "10.0.0.1" });
});

test("requestHeaders: bearer() is None for another scheme or a bare token", () => {
  for (const auth of ["Basic YWJj", "tok", "Bearer", "Bearer a b"]) {
    const h = requestHeaders(new Request("https://x/", { headers: { authorization: auth } }));
    assert.deepEqual(h.bearer(), { tag: "None" }, auth);
  }
});

test("requestHeaders: test lines split at the first colon; others are ignored", () => {
  const h = requestHeaders(["Authorization: Bearer abc", "X-Url: https://a:1/", "junk", ": empty"]);
  assert.deepEqual(h.bearer(), { tag: "Some", value: "abc" });
  assert.deepEqual(h.get("x-url"), { tag: "Some", value: "https://a:1/" });
  assert.deepEqual(requestHeaderPairs(h), { authorization: "Bearer abc", "x-url": "https://a:1/" });
  assert.deepEqual(requestHeaderPairs(requestHeaders([])), {});
});

test("httpResultToResponse: RawError answers its own status, body and content type", async () => {
  const body = new TextEncoder().encode('{"error":"conflict"}');
  const res = httpResultToResponse(HttpResult.RawError(409, body, "application/json"), () => null);
  assert.equal(res.status, 409);
  assert.equal(res.headers.get("content-type"), "application/json");
  assert.equal(await res.text(), '{"error":"conflict"}');
});

// #2403 (L6): `RawCreated` is `Raw`'s writer at 201.
test("httpResultToResponse: RawCreated answers 201 with its bytes and content type, and no ETag", async () => {
  const line = '{"name":"money","vers":"1.0.0"}\n';
  const body = new TextEncoder().encode(line);
  // The `GET` dispatch's `weakEtag` opt-in: a write's answer still carries no validator.
  const res = httpResultToResponse(HttpResult.RawCreated(body, "application/json"), () => null, {
    weakEtag: true,
  });
  assert.equal(res.status, 201);
  assert.equal(res.headers.get("content-type"), "application/json");
  assert.equal(res.headers.get("etag"), null);
  assert.equal(await res.text(), line);
});

test("responseToHttpResult: a 201 reads as Created, whether or not its body is JSON", async () => {
  const asString = (j: JsonValue) =>
    typeof j === "string" ? ({ tag: "Ok", value: j } as const) : ({ tag: "Err", error: {} } as never);
  const json = await responseToHttpResult(
    new Response('"made"', { status: 201, headers: { "content-type": "application/json" } }),
    asString,
  );
  assert.deepEqual(json, { tag: "Created", value: "made" });
  const text = await responseToHttpResult(
    new Response("made, as text", { status: 201, headers: { "content-type": "text/plain" } }),
    asString,
  );
  assert.deepEqual(text, { tag: "Created", value: "made, as text" });
});

// #2341 (decision C): a router rejection rendered through `on rejected`.
const rawError = (status: number, text: string) =>
  HttpResult.RawError(status, new TextEncoder().encode(text), "text/plain");

test("renderRejection: a RawError of the rejection's status replaces the body", async () => {
  const seen: string[] = [];
  const res = await renderRejection(
    new Response(JSON.stringify({ kind: "PayloadTooLarge", details: "too big" }), {
      status: 413,
      headers: { "content-type": "application/json", "x-keep": "1" },
    }),
    async (why) => {
      seen.push(`${why.tag}:${why.detail}`);
      return rawError(413, "too large");
    },
  );
  assert.deepEqual(seen, ["PayloadTooLarge:too big"]);
  assert.equal(res.status, 413);
  assert.equal(res.headers.get("content-type"), "text/plain");
  assert.equal(res.headers.get("x-keep"), "1");
  assert.equal(await res.text(), "too large");
});

test("renderRejection: a 405 keeps its Allow header; a 401 is Unauthorized", async () => {
  const tags: string[] = [];
  const hook = async (why: { tag: string }) => {
    tags.push(why.tag);
    return rawError(why.tag === "Unauthorized" ? 401 : 405, "no");
  };
  const r405 = await renderRejection(new Response(null, { status: 405, headers: { allow: "GET" } }), hook);
  const r401 = await renderRejection(new Response(null, { status: 401 }), hook);
  assert.deepEqual(tags, ["MethodNotAllowed", "Unauthorized"]);
  assert.equal(r405.headers.get("allow"), "GET");
  assert.equal(await r405.text(), "no");
  assert.equal(r401.status, 401);
});

test("renderRejection: another status, another variant or a fault sends the default", async () => {
  const original = () =>
    new Response(JSON.stringify({ kind: "MalformedJson", details: "bad" }), { status: 400 });
  const warn = console.warn;
  const error = console.error;
  console.warn = () => {};
  console.error = () => {};
  try {
    for (const hook of [
      async () => rawError(409, "x"),
      async () => HttpResult.BadRequest("x"),
      async () => {
        throw new Error("boom");
      },
    ]) {
      const res = await renderRejection(original(), hook);
      assert.equal(res.status, 400);
      assert.deepEqual(await res.json(), { kind: "MalformedJson", details: "bad" });
    }
  } finally {
    console.warn = warn;
    console.error = error;
  }
});

test("renderRejection: a response that is not a rejection passes through unasked", async () => {
  let called = false;
  const res = await renderRejection(new Response(null, { status: 204 }), async () => {
    called = true;
    return rawError(204, "");
  });
  assert.equal(called, false);
  assert.equal(res.status, 204);
});
