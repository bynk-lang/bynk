// #4610: the generated client refuses to send a credential over cleartext `http:` or `ws:`,
// except to a loopback host. The check is on the parsed URL's hostname.
import { test } from "node:test";
import assert from "node:assert/strict";
import { __call, __cleartextRemote, CleartextCredential, viaBinding, type ClientOptions } from "../src/client/http_client.ts";
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

test("an anonymous call, or one with an empty credential header, is unaffected over a remote http://", async () => {
  const anon = recorder();
  const out = await __call({ baseUrl: "http://api.example.com", fetch: anon.fetch }, undefined, route, build);
  assert.deepEqual(out, { status: 200, kind: "Ok" });
  assert.equal(anon.sent.length, 1);
  // An empty authorization or cookie header is no credential.
  for (const headers of [{ authorization: "" }, { cookie: "" }] as Record<string, string>[]) {
    const empty = recorder();
    await __call({ baseUrl: "http://api.example.com", fetch: empty.fetch, headers }, undefined, route, build);
    assert.equal(empty.sent.length, 1, JSON.stringify(headers));
  }
});

// #4631: an explicitly set `Cookie` is a credential like `Authorization`.
const session = { cookie: "sid=s3ssion-secret" };

test("an explicit Cookie to a remote http:// is refused and nothing is sent, wherever it was set", async () => {
  const where: [string, (r: ReturnType<typeof recorder>) => Promise<unknown>][] = [
    ["client headers", (r) => __call({ baseUrl: "http://api.example.com", fetch: r.fetch, headers: session }, undefined, route, build)],
    ["a header function", (r) => __call({ baseUrl: "http://api.example.com", fetch: r.fetch, headers: async () => session }, undefined, route, build)],
    ["the call's init", (r) => __call({ baseUrl: "http://api.example.com", fetch: r.fetch }, { headers: session }, route, build)],
    ["a typed header", (r) => __call({ baseUrl: "http://api.example.com", fetch: r.fetch }, undefined, route, () => Ok({ headers: session }))],
    ["typed cookies", (r) => __call({ baseUrl: "http://api.example.com", fetch: r.fetch }, undefined, route, () => Ok({ cookies: "sid=s3ssion-secret" }))],
  ];
  for (const [name, call] of where) {
    const r = recorder();
    await assert.rejects(
      () => call(r),
      (e: unknown) => {
        assert.ok(e instanceof CleartextCredential, name);
        assert.ok(!e.message.includes("s3ssion-secret"), `the error does not echo the cookie (${name})`);
        return true;
      },
      name,
    );
    assert.deepEqual(r.sent, [], `${name} sends nothing`);
  }
});

test("an explicit Cookie goes to loopback over http://, and to anywhere over https://", async () => {
  for (const base of ["http://localhost:8787", "http://127.0.0.1:8787", "http://[::1]:8787", "http://app.localhost", "https://api.example.com"]) {
    const r = recorder();
    const out = await __call({ baseUrl: base, fetch: r.fetch, headers: session }, undefined, route, build);
    assert.deepEqual(out, { status: 200, kind: "Ok" }, base);
    assert.equal(r.sent.length, 1, base);
  }
  // A lookalike host is remote.
  const r = recorder();
  await assert.rejects(
    () => __call({ baseUrl: "http://localhost.evil.com", fetch: r.fetch, headers: session }, undefined, route, build),
    CleartextCredential,
  );
  assert.deepEqual(r.sent, []);
});

test("a browser's own cookie jar is unaffected: no header is set, so nothing is checked", async () => {
  // A page cannot set `Cookie`; it sends `credentials: "include"` and the browser attaches its jar.
  const had = Object.getOwnPropertyDescriptor(globalThis, "document");
  try {
    Object.defineProperty(globalThis, "document", { value: {}, configurable: true });
    const seen: { credentials?: RequestCredentials; cookie: string | null }[] = [];
    const fetch = (async (_url: unknown, init?: RequestInit) => {
      seen.push({ credentials: init?.credentials, cookie: new Headers(init?.headers).get("cookie") });
      return new Response(null, { status: 200 });
    }) as typeof globalThis.fetch;
    const out = await __call({ baseUrl: "http://api.example.com", fetch, credentials: "include" }, undefined, route, build);
    assert.deepEqual(out, { status: 200, kind: "Ok" });
    assert.deepEqual(seen, [{ credentials: "include", cookie: null }]);
  } finally {
    if (had === undefined) Reflect.deleteProperty(globalThis, "document");
    else Object.defineProperty(globalThis, "document", had);
  }
});

test("a Service Binding call with a Cookie is not checked either", async () => {
  const seen: string[] = [];
  const binding = {
    async fetch(input: unknown) {
      seen.push(String(input));
      return new Response(null, { status: 200 });
    },
  };
  const out = await __call({ ...viaBinding(binding, "http://orders"), headers: session }, undefined, route, build);
  assert.deepEqual(out, { status: 200, kind: "Ok" });
  assert.deepEqual(seen, ["http://orders/things"]);
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

test("a Service Binding call is not checked: it never reaches the network, whatever baseUrl says", async () => {
  const seen: string[] = [];
  const binding = {
    async fetch(input: unknown) {
      seen.push(String(input));
      return new Response(null, { status: 200 });
    },
  };
  // `viaBinding(env.API, "http://orders")` with a forwarded Authorization header is not exposed.
  const out = await __call({ ...viaBinding(binding, "http://orders"), headers: bearer }, undefined, route, build);
  assert.deepEqual(out, { status: 200, kind: "Ok" });
  assert.deepEqual(seen, ["http://orders/things"]);
  // The same URL through any other `fetch` is still refused: the skip is for the binding's own fetch only.
  const r = recorder();
  await assert.rejects(
    () => __call({ baseUrl: "http://orders", fetch: r.fetch, headers: bearer }, undefined, route, build),
    CleartextCredential,
  );
  // Nothing a caller sets on the options switches the check off.
  await assert.rejects(
    () => __call({ baseUrl: "http://orders", fetch: r.fetch, headers: bearer, local: true } as ClientOptions, undefined, route, build),
    CleartextCredential,
  );
  assert.deepEqual(r.sent, []);
});

test("the error prints neither a userinfo password nor a query value", async () => {
  const r = recorder();
  const options: ClientOptions = {
    baseUrl: "http://alice:hunter2@api.example.com:8080/v1",
    fetch: r.fetch,
    headers: bearer,
  };
  await assert.rejects(
    () => __call(options, undefined, route, () => Ok({ query: "?api_key=sekret-value&x=1" })),
    (e: unknown) => {
      assert.ok(e instanceof CleartextCredential);
      for (const secret of ["hunter2", "alice", "sekret-value", "api_key", "s3cret-token"]) {
        assert.ok(!e.message.includes(secret), `the message does not print ${secret}: ${e.message}`);
        assert.ok(!e.url.includes(secret), `the url property does not hold ${secret}: ${e.url}`);
      }
      assert.equal(e.url, "http://api.example.com:8080/v1/things", "scheme, host, port and path are kept");
      assert.match(e.message, /http:\/\/api\.example\.com:8080\/v1\/things/);
      return true;
    },
  );
  assert.deepEqual(r.sent, []);
});
