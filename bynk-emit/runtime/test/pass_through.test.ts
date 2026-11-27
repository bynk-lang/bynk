// #3752 (pass-through HTTP, slice 3): the runtime half of an `on ANY(…)` route —
// the request view, the upstream exchange and the answer passed through. Each test
// names the row of the track's §8 threat model, or the decision, it holds.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  applyPassThroughSecurity,
  forwarded,
  inboundRequest,
  passThroughOwns,
  passThroughRefuses,
  passThroughResponse,
  passThroughYields,
  requestHeaders,
  upstreamForward,
  upstreamPathProblem,
  upstreamUrl,
} from "../src/http.ts";

const ORIGIN = "https://upstream.example";
const config = { origin: ORIGIN, timeoutSecs: 30 };

/** A fetcher that records what it was asked to send and answers `answer`. */
function recorder(answer: Response = new Response("ok")) {
  const sent: { url: string; init: RequestInit }[] = [];
  const fetcher = async (url: string, init: RequestInit) => {
    sent.push({ url, init });
    return answer;
  };
  return { sent, fetcher };
}

function view(url: string, init?: RequestInit) {
  const request = new Request(url, init);
  return inboundRequest(request, requestHeaders(request));
}

test("D11: an inbound request reads its method, raw query and headers", () => {
  const v = view("https://proxy.example/blog/x?a=1&b=%20", { method: "post", headers: { "x-k": "v" } });
  assert.equal(v.method(), "POST");
  assert.equal(v.query(), "a=1&b=%20");
  assert.deepEqual(v.headers().get("x-k"), { tag: "Some", value: "v" });
  assert.equal(view("https://proxy.example/").query(), "");
});

test("§8 SSRF (protocol-relative, backslash or user-information tricks): no path can change the host", async () => {
  for (const path of ["//other.example/x", "/\\other.example/x", "/@other.example/x", "/%2F%2Fother.example/x"]) {
    const { sent, fetcher } = recorder();
    const got = await upstreamForward(config, view("https://proxy.example/"), path, fetcher);
    if (got.tag === "Ok") {
      assert.equal(new URL(sent[0]!.url).origin, ORIGIN, path);
    } else {
      assert.deepEqual(got.error, { tag: "InvalidPath" }, path);
      assert.equal(sent.length, 0, path);
    }
  }
});

test("§8 SSRF: a path that doesn't start with `/` is refused, nothing sent", async () => {
  for (const path of ["@other.example/x", "other.example", "", ".other.example"]) {
    const { sent, fetcher } = recorder();
    const got = await upstreamForward(config, view("https://proxy.example/"), path, fetcher);
    assert.deepEqual(got, { tag: "Err", error: { tag: "InvalidPath" } }, path);
    assert.equal(sent.length, 0, path);
  }
});

test("§8 SSRF into the upstream's own private paths via `..`: dot segments are refused, encoded or not", () => {
  for (const path of [
    "/a/../admin",
    "/./x",
    "/%2e%2e/admin",
    "/%2E./admin",
    "/..%2F..%2Fadmin",
    "/..%5Cadmin",
    "/a%2F..%2Fb",
    "/a\\..\\admin",
    "/x/..",
  ]) {
    assert.notEqual(upstreamPathProblem(path), null, path);
  }
});

test("§8 SSRF: an encoded slash with no dot component is sent as received", () => {
  for (const path of ["/a%2Fb", "/r2/key%2Fwith%2Fslashes.png", "/a..b/.env", "/"]) {
    assert.equal(upstreamPathProblem(path), null, path);
  }
});

test("D13: `?`, `#` and control characters in a path are refused", () => {
  for (const path of ["/a?b", "/a#b", "/a\u0000b", "/a\nb", "/a\u007fb"]) {
    assert.notEqual(upstreamPathProblem(path), null, JSON.stringify(path));
  }
});

test("D13: the URL is built textually: origin, path, then `?` and the raw query", () => {
  assert.equal(upstreamUrl(ORIGIN, "/posts/x", ""), "https://upstream.example/posts/x");
  assert.equal(upstreamUrl(ORIGIN, "/posts/x", "a=%2F&b"), "https://upstream.example/posts/x?a=%2F&b");
});

test("D13, D15: forward sends the method, path, raw query and body, no inbound header, with manual redirects", async () => {
  const { sent, fetcher } = recorder();
  const v = view("https://proxy.example/blog/x?q=1", {
    method: "PUT",
    body: "payload",
    headers: { cookie: "s=1", authorization: "Bearer t", "x-forwarded-for": "6.6.6.6" },
  });
  const got = await upstreamForward(config, v, "/x", fetcher);
  assert.equal(got.tag, "Ok");
  assert.equal(sent.length, 1);
  assert.equal(sent[0]!.url, "https://upstream.example/x?q=1");
  assert.equal(sent[0]!.init.method, "PUT");
  assert.equal(sent[0]!.init.redirect, "manual");
  assert.equal(sent[0]!.init.headers, undefined, "slice 3 forwards no inbound header");
  assert.equal(await new Response(sent[0]!.init.body as ReadableStream).text(), "payload");
});

test("D15: GET and HEAD carry no body", async () => {
  for (const method of ["GET", "HEAD"]) {
    const { sent, fetcher } = recorder();
    await upstreamForward(config, view("https://proxy.example/", { method }), "/", fetcher);
    assert.equal(sent[0]!.init.body, undefined, method);
  }
});

test("D11: a second forward of one request faults", async () => {
  const { fetcher } = recorder();
  const v = view("https://proxy.example/");
  await upstreamForward(config, v, "/", fetcher);
  await assert.rejects(() => upstreamForward(config, v, "/", fetcher), /at most once/);
});

test("§8 oversized or slow upstream responses: `@timeout` bounds the head (`Err(Timeout)`)", async () => {
  const hang = (_: string, init: RequestInit) =>
    new Promise<Response>((_, reject) => init.signal!.addEventListener("abort", () => reject(new Error("aborted"))));
  const got = await upstreamForward({ origin: ORIGIN, timeoutSecs: 0.05 }, view("https://proxy.example/"), "/", hang);
  assert.deepEqual(got, { tag: "Err", error: { tag: "Timeout" } });
});

test("D19: an unreachable upstream is `Err(Unreachable)`", async () => {
  const refuse = async () => {
    throw new TypeError("fetch failed");
  };
  const got = await upstreamForward(config, view("https://proxy.example/"), "/", refuse);
  assert.deepEqual(got, { tag: "Err", error: { tag: "Unreachable" } });
});

test("D7: methods outside the seven, and an Upgrade (§8 WebSocket upgrade tunnelling), are refused", () => {
  assert.equal(passThroughRefuses(new Request("https://p.example/", { method: "PROPFIND" })), true);
  assert.equal(passThroughRefuses(new Request("https://p.example/", { headers: { upgrade: "websocket" } })), true);
  for (const method of ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]) {
    assert.equal(passThroughRefuses(new Request("https://p.example/", { method })), false, method);
  }
});

test("D5: an ANY route yields a path an ordinary route matches as specifically", () => {
  assert.equal(passThroughYields("/blog", ["/blog"]), true);
  assert.equal(passThroughYields("/blog/x", ["/blog"]), false);
  const owners: [string, string[]][] = [["/blog/*path", ["/blog"]]];
  assert.equal(passThroughOwns("/blog/x", owners), true);
  assert.equal(passThroughOwns("/blog", owners), false);
  assert.equal(passThroughOwns("/other", owners), false);
});

test("D9: a Forward passes the status, headers and body through, and synthesises nothing", async () => {
  const upstream = new Response("<p>hi</p>", {
    status: 404,
    headers: { "content-type": "text/html", etag: '"u1"', "cache-control": "public, max-age=60", "x-up": "1" },
  });
  const r = passThroughResponse({ tag: "Forward", response: forwarded(upstream) }, new Request("https://p.example/"));
  assert.equal(r.status, 404);
  assert.equal(r.headers.get("etag"), '"u1"');
  assert.equal(r.headers.get("cache-control"), "public, max-age=60");
  assert.equal(r.headers.get("x-up"), "1");
  assert.equal(await r.text(), "<p>hi</p>");
});

test("D9: a Forward of HEAD passes the upstream's HEAD answer, with no body invented", async () => {
  const upstream = new Response(null, { status: 200, headers: { "content-length": "12" } });
  const r = passThroughResponse({ tag: "Forward", response: forwarded(upstream) }, new Request("https://p.example/", { method: "HEAD" }));
  assert.equal(r.status, 200);
  assert.equal(r.body, null);
});

test("§8 credential leakage from the upstream: Set-Cookie and proxy-authenticate are stripped; hop-by-hop too", () => {
  const upstream = new Response("x", {
    headers: [
      ["set-cookie", "a=1; Domain=bynk-lang.org"],
      ["proxy-authenticate", "Basic"],
      ["connection", "close, x-drop"],
      ["keep-alive", "timeout=5"],
      ["x-drop", "1"],
      ["x-keep", "1"],
    ],
  });
  const r = passThroughResponse({ tag: "Forward", response: forwarded(upstream) }, new Request("https://p.example/"));
  for (const name of ["set-cookie", "proxy-authenticate", "connection", "keep-alive", "x-drop"]) {
    assert.equal(r.headers.has(name), false, name);
  }
  assert.equal(r.headers.get("x-keep"), "1");
});

test("D9: an upstream's own HSTS never reaches the proxy's origin; the policy's is stamped", () => {
  const policy = { nosniff: true, hstsMaxAgeSecs: null, htmlCsp: "default-src 'self'", htmlReferrerPolicy: "no-referrer" };
  const upstream = new Response("x", { headers: { "strict-transport-security": "max-age=1; includeSubDomains; preload" } });
  const r = applyPassThroughSecurity(
    passThroughResponse({ tag: "Forward", response: forwarded(upstream) }, new Request("https://p.example/")),
    policy,
  );
  assert.equal(r.headers.has("strict-transport-security"), false);
  assert.equal(r.headers.get("x-content-type-options"), "nosniff");
  const withHsts = applyPassThroughSecurity(new Response("x"), { ...policy, hstsMaxAgeSecs: 600 });
  assert.equal(withHsts.headers.get("strict-transport-security"), "max-age=600");
});

test("D9 (§8 an upstream serving script): HTML security headers are filled, not overwritten", () => {
  const policy = { nosniff: true, hstsMaxAgeSecs: null, htmlCsp: "default-src 'self'", htmlReferrerPolicy: "no-referrer" };
  const own = applyPassThroughSecurity(
    new Response("x", { headers: { "content-type": "text/html", "content-security-policy": "default-src *" } }),
    policy,
  );
  assert.equal(own.headers.get("content-security-policy"), "default-src *");
  assert.equal(own.headers.get("referrer-policy"), "no-referrer");
  const none = applyPassThroughSecurity(new Response("x", { headers: { "content-type": "text/html; charset=utf-8" } }), policy);
  assert.equal(none.headers.get("content-security-policy"), "default-src 'self'");
});

test("§7.4: an upstream 101 is answered 502", () => {
  const upstream = { status: 101, statusText: "", headers: new Headers(), body: null } as unknown as Response;
  const r = passThroughResponse({ tag: "Forward", response: forwarded(upstream) }, new Request("https://p.example/"));
  assert.equal(r.status, 502);
});

test("D8: a Respond answers locally, as an ordinary HttpResult", async () => {
  const r = passThroughResponse(
    { tag: "Respond", result: { tag: "PermanentRedirect", location: "/blog/" } as never },
    new Request("https://p.example/blog"),
  );
  assert.equal(r.status, 308);
  assert.equal(r.headers.get("location"), "/blog/");
});
