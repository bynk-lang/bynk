// `cloudflare.binding`: the `bynk.cloudflare` adapter's `WorkersKv` binding,
// tested against a KV namespace fake (standard-library S2, #1952), its
// `WorkersService` (interop I5, #1972) against a Service Binding fake, and its
// `WorkersR2` (standard-library P5, #2323) against an R2 bucket fake, and its
// `WorkersRateLimit` (standard-library P3, #2342) against a rate limiter fake.
//
// The fake follows Cloudflare's documented `KVNamespace` contract, not the
// binding: `get` resolves `null` for an absent key; `list` returns keys in
// lexicographic order, at most `limit` per page (1000 by default), with
// `list_complete: false` and a `cursor` while more remain, and no cursor on
// the last page; `put` takes `{ expirationTtl }` in seconds. Pages are small
// here so that draining several of them is exercised. Whether the fake matches
// the platform is what `bynkc/tests/binding_workerd.rs` checks, on `workerd`.

import { test } from "node:test";
import assert from "node:assert/strict";
import { WorkersKv, WorkersR2, WorkersRateLimit, WorkersService } from "./bynk/cloudflare.binding.js";
import { Method, FetchError } from "./bynk.js";
import { Some, None } from "./runtime.js";

function fakeKv(pageSize = 2) {
  const entries = new Map();
  const puts = [];
  return {
    entries,
    puts,
    async get(key) {
      return entries.has(key) ? entries.get(key) : null;
    },
    async put(key, value, options) {
      puts.push({ key, value, options });
      entries.set(key, value);
    },
    async delete(key) {
      entries.delete(key);
    },
    async list({ prefix, cursor, limit } = {}) {
      const keys = [...entries.keys()].filter((k) => !prefix || k.startsWith(prefix)).sort();
      const start = cursor === undefined ? 0 : Number(cursor);
      const size = Math.min(limit ?? 1000, pageSize);
      const page = keys.slice(start, start + size).map((name) => ({ name }));
      const next = start + size;
      return next < keys.length
        ? { keys: page, list_complete: false, cursor: String(next) }
        : { keys: page, list_complete: true };
    },
  };
}

const t = (claim, fn) => test(`cloudflare.binding ${claim}`, fn);

t("WorkersKv.get: an absent key is None, a present one Some", async () => {
  const kv = fakeKv();
  kv.entries.set("present", "v");
  const store = new WorkersKv({ KV: kv });
  assert.deepEqual(await store.get("absent"), None);
  assert.deepEqual(await store.get("present"), Some("v"));
});

t("WorkersKv.put: stores the value with no expiry option", async () => {
  const kv = fakeKv();
  const store = new WorkersKv({ KV: kv });
  await store.put("k", "v1");
  await store.put("k", "v2");
  assert.deepEqual(await store.get("k"), Some("v2"));
  assert.equal(kv.puts.length, 2);
  assert.equal(kv.puts[1].options, undefined);
});

t("WorkersKv.putTtl: passes the TTL through as expirationTtl seconds", async () => {
  const kv = fakeKv();
  const store = new WorkersKv({ KV: kv });
  await store.putTtl("k", "v", 120);
  assert.deepEqual(kv.puts, [{ key: "k", value: "v", options: { expirationTtl: 120 } }]);
  assert.deepEqual(await store.get("k"), Some("v"));
});

t("WorkersKv.delete: removes the key, and deleting an absent key is not an error", async () => {
  const kv = fakeKv();
  kv.entries.set("k", "v");
  const store = new WorkersKv({ KV: kv });
  await store.delete("k");
  assert.deepEqual(await store.get("k"), None);
  await store.delete("never-there");
});

t("WorkersKv.list: drains every page, filtered by prefix, in key order", async () => {
  const kv = fakeKv(2);
  for (const k of ["user:3", "user:1", "order:1", "user:2", "user:5", "user:4"]) {
    kv.entries.set(k, "x");
  }
  const store = new WorkersKv({ KV: kv });
  assert.deepEqual(await store.list(Some("user:")), ["user:1", "user:2", "user:3", "user:4", "user:5"]);
  assert.deepEqual(await store.list(None), ["order:1", "user:1", "user:2", "user:3", "user:4", "user:5"]);
  assert.deepEqual(await store.list(Some("nothing:")), []);
});

t("WorkersKv.get: without an env.KV binding, says so rather than answering", async () => {
  await assert.rejects(new WorkersKv({}).get("k"), /requires a KV namespace binding \(env\.KV\)/);
  await assert.rejects(new WorkersKv().get("k"), /env\.KV/);
});

// S4a (#1966): a derived capability's instance (`capability Sessions = Kv`)
// reads its own binding, named at construction; the bare `Kv` keeps `KV`.
t("WorkersKv: a named binding reads env[binding], apart from KV", async () => {
  const bare = fakeKv();
  const sessions = fakeKv();
  const env = { KV: bare, KV_SESSIONS: sessions };
  await new WorkersKv(env, "KV_SESSIONS").put("k", "instance");
  await new WorkersKv(env).put("k", "bare");
  assert.deepEqual(await new WorkersKv(env, "KV_SESSIONS").get("k"), Some("instance"));
  assert.deepEqual(await new WorkersKv(env).get("k"), Some("bare"));
  assert.equal(sessions.entries.get("k"), "instance");
  assert.equal(bare.entries.get("k"), "bare");
});

t("WorkersKv: a missing named binding names it", async () => {
  await assert.rejects(
    new WorkersKv({ KV: fakeKv() }, "KV_SESSIONS").get("k"),
    /requires a KV namespace binding \(env\.KV_SESSIONS\)/,
  );
});

// Interop I5 (#1972): a Service Binding fake following Cloudflare's
// `Fetcher` contract as the generated `Env` types it: `fetch(request)` with
// one `Request`, answering a `Response`. Records what it was sent.
function fakeService(answer = () => new Response("ok", { status: 200 })) {
  const sent = [];
  return {
    sent,
    async fetch(request) {
      sent.push({
        method: request.method,
        url: request.url,
        headers: Object.fromEntries(request.headers),
        body: request.body === null ? null : await request.text(),
      });
      return answer(request);
    },
  };
}

const serviceRequest = (overrides = {}) => ({
  method: Method.Get,
  url: "https://legacy-billing/charge",
  contentType: None,
  authorization: None,
  headers: new Map(),
  body: None,
  ...overrides,
});

t("WorkersService.send: sends over env[binding], not the network, and reads the response", async () => {
  const billing = fakeService(() => new Response("charged", { status: 201, headers: { etag: "v1" } }));
  const globalFetch = globalThis.fetch;
  globalThis.fetch = () => {
    throw new Error("the global fetch must not be used");
  };
  try {
    // The deployment's call secret (#2004) is on `env` beside the binding, and
    // must never travel to a Worker outside the project.
    const env = { SERVICE_BILLING: billing, BYNK_CALL_SECRET: "deployment-secret" };
    const res = await new WorkersService(env, "SERVICE_BILLING").send(
      serviceRequest({ method: Method.Post, contentType: Some("text/plain"), body: Some("a") }),
    );
    assert.equal(res.tag, "Ok");
    assert.equal(res.value.status, 201);
    assert.equal(res.value.body, "charged");
    assert.equal(res.value.headers.get("etag"), "v1");
  } finally {
    globalThis.fetch = globalFetch;
  }
  assert.equal(billing.sent.length, 1);
  assert.equal(billing.sent[0].method, "POST");
  assert.equal(billing.sent[0].url, "https://legacy-billing/charge");
  assert.equal(billing.sent[0].headers["content-type"], "text/plain");
  assert.equal(billing.sent[0].body, "a");
  assert.equal(billing.sent[0].headers["x-bynk-call-secret"], undefined);
  assert.ok(!JSON.stringify(billing.sent[0]).includes("deployment-secret"));
});

t("WorkersService.send: a status the Worker returns, 5xx included, is Ok", async () => {
  const billing = fakeService(() => new Response("down", { status: 503 }));
  const res = await new WorkersService({ SERVICE_BILLING: billing }, "SERVICE_BILLING").send(serviceRequest());
  assert.equal(res.tag, "Ok");
  assert.equal(res.value.status, 503);
});

t("WorkersService.send: a refused header is Err(InvalidHeader), and nothing is sent", async () => {
  const billing = fakeService();
  const res = await new WorkersService({ SERVICE_BILLING: billing }, "SERVICE_BILLING").send(
    serviceRequest({ headers: new Map([["host", "elsewhere"]]) }),
  );
  assert.deepEqual(res, { tag: "Err", error: FetchError.InvalidHeader });
  assert.equal(billing.sent.length, 0);
});

t("WorkersService.send: a call the binding fails is Err(Network)", async () => {
  const billing = {
    async fetch() {
      throw new TypeError("internal error");
    },
  };
  const res = await new WorkersService({ SERVICE_BILLING: billing }, "SERVICE_BILLING").send(serviceRequest());
  assert.deepEqual(res, { tag: "Err", error: FetchError.Network });
});

t("WorkersService.send: each instance reads its own binding", async () => {
  const billing = fakeService();
  const ledger = fakeService();
  const env = { SERVICE_BILLING: billing, SERVICE_LEDGER: ledger };
  await new WorkersService(env, "SERVICE_LEDGER").send(serviceRequest());
  assert.equal(billing.sent.length, 0);
  assert.equal(ledger.sent.length, 1);
});

t("WorkersService.send: a missing binding names it", async () => {
  await assert.rejects(
    new WorkersService({}, "SERVICE_BILLING").send(serviceRequest()),
    /requires a Service Binding \(env\.SERVICE_BILLING\)/,
  );
});

// P5 (#2323): `WorkersR2`, against a fake that follows Cloudflare's documented
// `R2Bucket` contract: `get` and `head` resolve `null` for an absent key;
// `put` resolves the stored object's metadata, or `null` when `onlyIf` failed
// (`etagMatches` needs the current etag, `etagDoesNotMatch: "*"` an absent
// key); `list` returns objects in key order, at most `limit` per page, with
// `truncated: true` and a `cursor` while more remain, and `httpMetadata` and
// `customMetadata` only when `include` asks for them. Pages are small here so
// that paging is exercised. Whether the fake matches the platform is what
// `bynkc/tests/binding_workerd.rs` checks, on `workerd`.
function fakeR2(pageSize = 2) {
  const objects = new Map();
  const puts = [];
  const lists = [];
  let version = 0;
  const meta = (o, include = []) => ({
    key: o.key,
    size: o.body.length,
    etag: o.etag,
    uploaded: o.uploaded,
    httpMetadata: include.includes("httpMetadata") ? o.httpMetadata : undefined,
    customMetadata: include.includes("customMetadata") ? o.customMetadata : undefined,
  });
  return {
    objects,
    puts,
    lists,
    async get(key) {
      const o = objects.get(key);
      if (!o) return null;
      return { ...meta(o, ["httpMetadata", "customMetadata"]), arrayBuffer: async () => o.body.slice().buffer };
    },
    async head(key) {
      const o = objects.get(key);
      return o ? meta(o, ["httpMetadata", "customMetadata"]) : null;
    },
    async put(key, value, options = {}) {
      puts.push({ key, options });
      const current = objects.get(key);
      const { etagMatches, etagDoesNotMatch } = options.onlyIf ?? {};
      if (etagMatches !== undefined && current?.etag !== etagMatches) return null;
      if (etagDoesNotMatch !== undefined && (etagDoesNotMatch === "*" ? current : current?.etag === etagDoesNotMatch)) {
        return null;
      }
      version += 1;
      const o = {
        key,
        body: new Uint8Array(value),
        etag: `etag-${version}`,
        uploaded: new Date(1_000 * version),
        httpMetadata: options.httpMetadata ?? {},
        customMetadata: options.customMetadata ?? {},
      };
      objects.set(key, o);
      return meta(o, ["httpMetadata", "customMetadata"]);
    },
    async delete(key) {
      objects.delete(key);
    },
    async list({ prefix, cursor, limit, include } = {}) {
      lists.push({ prefix, cursor, include });
      const keys = [...objects.keys()].filter((k) => !prefix || k.startsWith(prefix)).sort();
      const start = cursor === undefined ? 0 : Number(cursor);
      const size = Math.min(limit ?? 1000, pageSize);
      const page = keys.slice(start, start + size).map((k) => meta(objects.get(k), include));
      const next = start + size;
      return next < keys.length
        ? { objects: page, truncated: true, cursor: String(next) }
        : { objects: page, truncated: false };
    },
  };
}

const bytes = (...octets) => new Uint8Array(octets);
const noMetadata = () => new Map();
const putOptions = (over = {}) => ({
  contentType: None,
  metadata: noMetadata(),
  onlyIf: { tag: "Always" },
  ...over,
});

t("WorkersR2.get: an absent key is None, a present one Some of its bytes", async () => {
  const r2 = fakeR2();
  await r2.put("k", bytes(1, 2, 3));
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  assert.deepEqual(await store.get("absent"), None);
  const got = await store.get("k");
  assert.equal(got.tag, "Some");
  assert.ok(got.value instanceof Uint8Array);
  assert.deepEqual([...got.value], [1, 2, 3]);
});

t("WorkersR2.head: reports the object without its body, None when absent", async () => {
  const r2 = fakeR2();
  await r2.put("k", bytes(1, 2, 3), { httpMetadata: { contentType: "text/plain" }, customMetadata: { a: "1" } });
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  assert.deepEqual(await store.head("absent"), None);
  const head = await store.head("k");
  assert.equal(head.tag, "Some");
  assert.deepEqual(
    { ...head.value, metadata: [...head.value.metadata] },
    {
      key: "k",
      size: 3,
      etag: "etag-1",
      uploaded: 1_000,
      contentType: Some("text/plain"),
      metadata: [["a", "1"]],
    },
  );
});

t("WorkersR2.put: stores unconditionally, replacing, and returns the new object", async () => {
  const r2 = fakeR2();
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  const first = await store.put("k", bytes(1));
  const second = await store.put("k", bytes(2, 2));
  assert.equal(first.etag, "etag-1");
  assert.equal(second.etag, "etag-2");
  assert.equal(second.size, 2);
  assert.deepEqual(second.contentType, None);
  assert.equal(second.metadata.size, 0);
  assert.deepEqual([...(await store.get("k")).value], [2, 2]);
});

t("WorkersR2.putWith: content type and metadata are stored with the object", async () => {
  const r2 = fakeR2();
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  const stored = await store.putWith(
    "k",
    bytes(7),
    putOptions({ contentType: Some("image/png"), metadata: new Map([["origin", "test"]]) }),
  );
  assert.equal(stored.tag, "Some");
  assert.deepEqual(stored.value.contentType, Some("image/png"));
  assert.deepEqual([...stored.value.metadata], [["origin", "test"]]);
  assert.deepEqual(r2.puts[0].options.httpMetadata, { contentType: "image/png" });
  assert.deepEqual(r2.puts[0].options.customMetadata, { origin: "test" });
  assert.equal(r2.puts[0].options.onlyIf, undefined);
});

t("WorkersR2.putWith: Absent creates only, and an existing key is None, unchanged", async () => {
  const r2 = fakeR2();
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  const absent = putOptions({ onlyIf: { tag: "Absent" } });
  assert.equal((await store.putWith("k", bytes(1), absent)).tag, "Some");
  assert.deepEqual(await store.putWith("k", bytes(2), absent), None);
  assert.deepEqual([...(await store.get("k")).value], [1]);
  assert.deepEqual(r2.puts[0].options.onlyIf, { etagDoesNotMatch: "*" });
});

t("WorkersR2.putWith: EtagMatches swaps only the current version; otherwise None, unchanged", async () => {
  const r2 = fakeR2();
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  const v1 = await store.put("k", bytes(1));
  const stale = await store.putWith("k", bytes(2), putOptions({ onlyIf: { tag: "EtagMatches", etag: "nope" } }));
  assert.deepEqual(stale, None);
  const fresh = await store.putWith("k", bytes(3), putOptions({ onlyIf: { tag: "EtagMatches", etag: v1.etag } }));
  assert.equal(fresh.tag, "Some");
  assert.deepEqual([...(await store.get("k")).value], [3]);
  const missing = await store.putWith("gone", bytes(1), putOptions({ onlyIf: { tag: "EtagMatches", etag: v1.etag } }));
  assert.deepEqual(missing, None);
});

t("WorkersR2.delete: removes the key, and deleting an absent key is not an error", async () => {
  const r2 = fakeR2();
  await r2.put("k", bytes(1));
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  await store.delete("k");
  await store.delete("k");
  assert.deepEqual(await store.get("k"), None);
});

t("WorkersR2.list: one page at a time, filtered by prefix, with the cursor of the next", async () => {
  const r2 = fakeR2();
  for (const key of ["p/3", "p/1", "q/1", "p/2"]) await r2.put(key, bytes(1));
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  const first = await store.list(Some("p/"), None);
  assert.deepEqual(first.keys.map((o) => o.key), ["p/1", "p/2"]);
  assert.equal(first.cursor.tag, "Some");
  const second = await store.list(Some("p/"), first.cursor);
  assert.deepEqual(second.keys.map((o) => o.key), ["p/3"]);
  assert.deepEqual(second.cursor, None);
  assert.deepEqual(r2.lists[1].cursor, first.cursor.value);
  const all = await store.list(None, None);
  assert.deepEqual(all.keys.map((o) => o.key), ["p/1", "p/2"]);
});

t("WorkersR2.list: each object carries its metadata", async () => {
  const r2 = fakeR2();
  await r2.put("k", bytes(1), { httpMetadata: { contentType: "text/plain" }, customMetadata: { a: "1" } });
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  const page = await store.list(None, None);
  assert.deepEqual(page.keys[0].contentType, Some("text/plain"));
  assert.deepEqual([...page.keys[0].metadata], [["a", "1"]]);
  assert.deepEqual(page.keys[0].uploaded, 1_000);
});

t("WorkersR2: each instance reads its own binding", async () => {
  const archives = fakeR2();
  const indexes = fakeR2();
  const env = { R2_ARCHIVES: archives, R2_INDEXES: indexes };
  await new WorkersR2(env, "R2_INDEXES").put("k", bytes(1));
  assert.equal(archives.objects.size, 0);
  assert.equal(indexes.objects.size, 1);
});

t("WorkersR2.get: a missing binding names it", async () => {
  await assert.rejects(new WorkersR2({}, "R2_ARCHIVES").get("k"), /requires an R2 bucket binding \(env\.R2_ARCHIVES\)/);
});

// P3 (#2342): `WorkersRateLimit`, against a fake that follows Cloudflare's
// documented rate limiting binding: `limit({ key })` resolves `{ success }`,
// `false` once `key` is over the binding's configured limit in the window.
function fakeRateLimit(limit) {
  const counts = new Map();
  const keys = [];
  return {
    keys,
    async limit({ key }) {
      keys.push(key);
      const n = (counts.get(key) ?? 0) + 1;
      counts.set(key, n);
      return { success: n <= limit };
    },
  };
}

t("WorkersRateLimit.allow: true within the limit, false over it, per key", async () => {
  const rl = fakeRateLimit(2);
  const limiter = new WorkersRateLimit({ RATELIMIT_LOGIN: rl }, "RATELIMIT_LOGIN");
  assert.deepEqual(
    [await limiter.allow("a"), await limiter.allow("a"), await limiter.allow("a"), await limiter.allow("b")],
    [true, true, false, true],
  );
  assert.deepEqual(rl.keys, ["a", "a", "a", "b"]);
});

t("WorkersRateLimit: each instance reads its own binding", async () => {
  const login = fakeRateLimit(1);
  const publish = fakeRateLimit(1);
  const env = { RATELIMIT_LOGIN: login, RATELIMIT_PUBLISH: publish };
  await new WorkersRateLimit(env, "RATELIMIT_PUBLISH").allow("k");
  assert.equal(login.keys.length, 0);
  assert.equal(publish.keys.length, 1);
});

t("WorkersRateLimit.allow: a missing binding names it", async () => {
  await assert.rejects(
    new WorkersRateLimit({}, "RATELIMIT_LOGIN").allow("k"),
    /requires a rate limiting binding \(env\.RATELIMIT_LOGIN\)/,
  );
});
