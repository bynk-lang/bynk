// #4610: the generated client refuses to send a credential over cleartext `http:` or `ws:`,
// except to a loopback host. The check is on the parsed URL's hostname.
import { test } from "node:test";
import assert from "node:assert/strict";
import { __call, __cleartextRemote, CleartextCredential, type ClientOptions } from "../src/client/http_client.ts";
import { Ok } from "../src/result.ts";

const route = { method: "GET", path: "/things", replies: { 200: [{ kind: "Ok", body: "none" as const }] } };
const build = () => Ok({});

/** A `fetch` that records the URLs it was asked for. */
function recorder(): { fetch: typeof globalThis.fetch; sent: string[] } {
  const sent: string[] = [];
  const fetch = (async (input: unknown) => {
    sent.push(String(input));
    return new Response(null, { status: 200 });
  }) as typeof globalThis.fetch;
  return { fetch, sent };
}

const bearer = { authorization: "Bearer s3cret-token" };

test("a bearer token over a remote http:// is refused and nothing is sent", async () => {
  const r = recorder();
  const options: ClientOptions = { baseUrl: "http://api.example.com", fetch: r.fetch, headers: bearer };
  await assert.rejects(
    () => __call(options, undefined, route, build),
    (e: unknown) => {
      assert.ok(e instanceof CleartextCredential);
      assert.equal(e.url, "http://api.example.com/things");
      assert.match(e.message, /http:\/\/api\.example\.com\/things/);
      assert.match(e.message, /https:\/\//);
      assert.ok(!e.message.includes("s3cret-token"), "the error does not echo the credential");
      return true;
    },
  );
  assert.deepEqual(r.sent, [], "nothing was sent");
});

test("the credential is found wherever it was set: a header function, the call's init, a typed header", async () => {
  for (const make of [
    (r: ReturnType<typeof recorder>) => ({ options: { baseUrl: "http://api.example.com", fetch: r.fetch, headers: async () => bearer } as ClientOptions }),
    (r: ReturnType<typeof recorder>) => ({
      options: { baseUrl: "http://api.example.com", fetch: r.fetch } as ClientOptions,
      init: { headers: bearer },
    }),
  ]) {
    const r = recorder();
    const { options, init } = make(r) as { options: ClientOptions; init?: RequestInit };
    await assert.rejects(() => __call(options, init, route, build), CleartextCredential);
    assert.deepEqual(r.sent, []);
  }
  const r = recorder();
  await assert.rejects(
    () => __call({ baseUrl: "http://api.example.com", fetch: r.fetch }, undefined, route, () => Ok({ headers: bearer })),
    CleartextCredential,
  );
  assert.deepEqual(r.sent, []);
});

test("a bearer token goes to loopback over http://, and to anywhere over https://", async () => {
  for (const base of [
    "http://localhost:8787",
    "http://localhost",
    "http://localhost.",
    "http://app.localhost:3000",
    "http://127.0.0.1:8787",
    "http://127.255.255.254",
    "http://127.1",
    "http://[::1]:8787",
    "https://api.example.com",
  ]) {
    const r = recorder();
    const out = await __call({ baseUrl: base, fetch: r.fetch, headers: bearer }, undefined, route, build);
    assert.deepEqual(out, { status: 200, kind: "Ok" }, base);
    assert.equal(r.sent.length, 1, `${base} sends`);
  }
});

test("a lookalike host is remote: the test is on the parsed hostname, not a prefix", async () => {
  for (const base of [
    "http://localhost.evil.com",
    "http://127.0.0.1.evil.com",
    "http://evil.com/localhost",
    "http://evil.com/127.0.0.1",
    "http://localhost@evil.com",
    "http://127.0.0.1@evil.com",
    "http://evil.com:80@evil.org",
    "http://notlocalhost",
    "http://128.0.0.1",
    "http://126.255.255.255",
    "http://[::2]",
    "http://[::ffff:7f00:1]",
    "http://10.0.0.1",
  ]) {
    const r = recorder();
    await assert.rejects(
      () => __call({ baseUrl: base, fetch: r.fetch, headers: bearer }, undefined, route, build),
      CleartextCredential,
      base,
    );
    assert.deepEqual(r.sent, [], `${base} sends nothing`);
  }
});

test("an anonymous call, or one with a cookie only, is unaffected over a remote http://", async () => {
  const anon = recorder();
  const out = await __call({ baseUrl: "http://api.example.com", fetch: anon.fetch }, undefined, route, build);
  assert.deepEqual(out, { status: 200, kind: "Ok" });
  assert.equal(anon.sent.length, 1);
  // Cookie credentials are the browser's to govern (`Secure`), not this check's.
  const cookie = recorder();
  const withCookie = await __call({ baseUrl: "http://api.example.com", fetch: cookie.fetch }, undefined, route, () =>
    Ok({ cookies: "sid=abc" }),
  );
  assert.deepEqual(withCookie, { status: 200, kind: "Ok" });
  assert.equal(cookie.sent.length, 1);
  // An empty authorization header is no credential.
  const empty = recorder();
  await __call({ baseUrl: "http://api.example.com", fetch: empty.fetch, headers: { authorization: "" } }, undefined, route, build);
  assert.equal(empty.sent.length, 1);
});

test("a relative base URL is resolved against location.origin and checked the same way", async () => {
  const had = Object.getOwnPropertyDescriptor(globalThis, "location");
  try {
    for (const [origin, refused] of [
      ["http://app.example.com", true],
      ["https://app.example.com", false],
      ["http://localhost:5173", false],
    ] as const) {
      Object.defineProperty(globalThis, "location", { value: { origin }, configurable: true });
      const r = recorder();
      const call = __call({ baseUrl: "/api", fetch: r.fetch, headers: bearer }, undefined, route, build);
      if (refused) {
        await assert.rejects(() => call, CleartextCredential, origin);
        assert.deepEqual(r.sent, []);
      } else {
        await call;
        assert.equal(r.sent.length, 1, origin);
      }
    }
  } finally {
    if (had === undefined) Reflect.deleteProperty(globalThis, "location");
    else Object.defineProperty(globalThis, "location", had);
  }
});

test("the shared test serves a ws: URL as it serves an http: one", () => {
  assert.equal(__cleartextRemote("ws://api.example.com/socket"), true);
  assert.equal(__cleartextRemote("ws://localhost:8787/socket"), false);
  assert.equal(__cleartextRemote("ws://[::1]/socket"), false);
  assert.equal(__cleartextRemote("ws://localhost.evil.com/socket"), true);
  assert.equal(__cleartextRemote("wss://api.example.com/socket"), false);
  assert.equal(__cleartextRemote("https://api.example.com"), false);
  assert.equal(__cleartextRemote("not a url"), false, "an unparseable URL is left to fetch to reject");
});
