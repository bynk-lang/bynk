// `cloudflare.binding`: the `bynk.cloudflare` adapter's `WorkersKv` binding,
// tested against a KV namespace fake (standard-library S2, #1952), its
// `WorkersService` (interop I5, #1972) against a Service Binding fake, and its
// `WorkersR2` (standard-library P5, #2323; streams and multipart, P5b, #2626)
// against an R2 bucket fake, and its
// `WorkersRateLimit` (standard-library P3, #2342) against a rate limiter fake,
// `WorkersQueue` (standard-library P2, #2578) against a queue producer fake,
// `WorkersAnalytics` (standard-library P3b, #2582) against a dataset fake,
// `WorkersD1` (standard-library P4a, #2602) against a database fake, and
// `WorkersEdgeCache` (L7, #2414) against an execution context fake.
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
import { WorkersAnalytics, WorkersD1, WorkersEdgeCache, WorkersKv, WorkersQueue, WorkersR2, WorkersRateLimit, WorkersService } from "./bynk/cloudflare.binding.js";
import { Method, FetchError } from "./bynk.js";
import { Some, None } from "./__bynk_runtime/result.js";

function fakeKv(pageSize = 2) {
  // Values are text or bytes, as KV stores them; `meta` and `expiry` sit
  // beside them (P1, #2590), and `reads` records each read's options.
  const entries = new Map();
  const meta = new Map();
  const expiry = new Map();
  const puts = [];
  const reads = [];
  const bytesOf = (v) => (typeof v === "string" ? new TextEncoder().encode(v) : new Uint8Array(v));
  return {
    entries,
    meta,
    puts,
    reads,
    async get(key, type) {
      reads.push({ key, type });
      if (!entries.has(key)) return null;
      const v = entries.get(key);
      if (type === "arrayBuffer") return bytesOf(v).slice().buffer;
      return typeof v === "string" ? v : new TextDecoder().decode(v);
    },
    async getWithMetadata(key, options) {
      reads.push({ key, options });
      if (!entries.has(key)) return { value: null, metadata: null };
      const v = entries.get(key);
      return { value: typeof v === "string" ? v : new TextDecoder().decode(v), metadata: meta.get(key) ?? null };
    },
    async put(key, value, options) {
      puts.push({ key, value, options });
      entries.set(key, value);
      if (options?.metadata !== undefined) meta.set(key, options.metadata);
      if (options?.expiration !== undefined) expiry.set(key, options.expiration);
    },
    async delete(key) {
      entries.delete(key);
    },
    async list({ prefix, cursor, limit } = {}) {
      const keys = [...entries.keys()].filter((k) => !prefix || k.startsWith(prefix)).sort();
      const start = cursor === undefined ? 0 : Number(cursor);
      const size = Math.min(limit ?? 1000, pageSize);
      const page = keys.slice(start, start + size).map((name) => ({
        name,
        ...(expiry.has(name) ? { expiration: expiry.get(name) } : {}),
        ...(meta.has(name) ? { metadata: meta.get(name) } : {}),
      }));
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
  const uploads = new Map();
  const completions = [];
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
      // P5b: the body streams two bytes a chunk, so chunking is exercised;
      // `cancelled` records a reader that stopped early.
      let at = 0;
      const body = new ReadableStream({
        pull(controller) {
          if (at >= o.body.length) return controller.close();
          controller.enqueue(o.body.slice(at, at + 2));
          at += 2;
        },
        cancel() {
          o.cancelled = true;
        },
      });
      return { ...meta(o, ["httpMetadata", "customMetadata"]), body, arrayBuffer: async () => o.body.slice().buffer };
    },
    async head(key) {
      const o = objects.get(key);
      return o ? meta(o, ["httpMetadata", "customMetadata"]) : null;
    },
    async put(key, value, options = {}) {
      puts.push({ key, options, streamed: value instanceof ReadableStream });
      // P5b: a stream is read whole, as R2 reads it; one that errors fails
      // the put, and nothing is stored.
      if (value instanceof ReadableStream) value = new Uint8Array(await new Response(value).arrayBuffer());
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
    // P5b: multipart, as R2 documents it. `resumeMultipartUpload` checks
    // nothing; an unknown upload fails at its first operation. `complete`
    // takes the parts' numbers and etags and joins them in the order listed,
    // as Miniflare does (`completions` records that order).
    uploads,
    completions,
    async createMultipartUpload(key, options = {}) {
      version += 1;
      const uploadId = `upload-${version}`;
      uploads.set(uploadId, { key, options, parts: new Map() });
      return this.resumeMultipartUpload(key, uploadId);
    },
    resumeMultipartUpload(key, uploadId) {
      const open = () => {
        const u = uploads.get(uploadId);
        if (!u || u.key !== key) throw new Error("The specified multipart upload does not exist.");
        return u;
      };
      return {
        key,
        uploadId,
        async uploadPart(partNumber, value) {
          const u = open();
          version += 1;
          u.parts.set(partNumber, { etag: `part-${version}`, body: new Uint8Array(value) });
          return { partNumber, etag: `part-${version}` };
        },
        async complete(parts) {
          const u = open();
          completions.push(parts.map((p) => p.partNumber));
          const bodies = parts
            .map((p) => {
              const part = u.parts.get(p.partNumber);
              if (!part || part.etag !== p.etag) throw new Error("One or more of the specified parts could not be found.");
              return part.body;
            });
          const body = new Uint8Array(bodies.flatMap((b) => [...b]));
          uploads.delete(uploadId);
          version += 1;
          const o = {
            key,
            body,
            etag: `etag-${version}`,
            uploaded: new Date(1_000 * version),
            httpMetadata: u.options.httpMetadata ?? {},
            customMetadata: u.options.customMetadata ?? {},
          };
          objects.set(key, o);
          return meta(o, ["httpMetadata", "customMetadata"]);
        },
        async abort() {
          open();
          uploads.delete(uploadId);
        },
      };
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

// P5b (#2626): a Bynk `Stream[Bytes]`, an async iterable of chunks.
async function* chunks(...parts) {
  for (const p of parts) yield new Uint8Array(p);
}

t("WorkersR2.getStream: None when absent; the object's metadata and its body chunk by chunk", async () => {
  const r2 = fakeR2();
  await r2.put("k", bytes(1, 2, 3, 4, 5), { httpMetadata: { contentType: "video/mp4" }, customMetadata: { by: "me" } });
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  assert.deepEqual(await store.getStream("absent"), None);
  const got = await store.getStream("k");
  assert.equal(got.tag, "Some");
  assert.equal(got.value.object.size, 5);
  assert.deepEqual(got.value.object.contentType, Some("video/mp4"));
  assert.equal(got.value.object.metadata.get("by"), "me");
  const seen = [];
  for await (const c of got.value.body) seen.push([...c]);
  assert.deepEqual(seen, [[1, 2], [3, 4], [5]]);
});

t("WorkersR2.getStream: the body is unclaimed until read, and stopping early cancels R2's stream", async () => {
  const r2 = fakeR2();
  await r2.put("k", bytes(1, 2, 3, 4, 5));
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  const { value } = await store.getStream("k");
  assert.equal(value.body[Symbol.for("bynk.stream.read")], undefined);
  for await (const c of value.body) {
    assert.deepEqual([...c], [1, 2]);
    break;
  }
  assert.equal(r2.objects.get("k").cancelled, true);
});

t("WorkersR2.putStream: writes a stream of the declared size, as a stream, and returns the object", async () => {
  const r2 = fakeR2();
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  const o = await store.putStream("k", chunks([1, 2], [], [3]), 3);
  assert.equal(o.size, 3);
  assert.equal(r2.puts[0].streamed, true);
  assert.deepEqual([...r2.objects.get("k").body], [1, 2, 3]);
});

t("WorkersR2.putStream: a stream longer or shorter than its size faults, and nothing is stored", async () => {
  const r2 = fakeR2();
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  await assert.rejects(store.putStream("long", chunks([1, 2], [3]), 2), /the stream held at least 3 bytes, not the declared size 2/);
  await assert.rejects(store.putStream("short", chunks([1]), 2), /the stream held 1 bytes, not the declared size 2/);
  await assert.rejects(store.putStream("neg", chunks(), -1), /size must be a whole number of bytes, not -1/);
  assert.equal(r2.objects.size, 0);
});

t("WorkersR2.putStream: claims its stream, so a stream already read faults", async () => {
  const store = new WorkersR2({ R2_ARCHIVES: fakeR2() }, "R2_ARCHIVES");
  const s = chunks([1]);
  await store.putStream("a", s, 1);
  await assert.rejects(store.putStream("b", s, 1), /a stream can be read only once/);
});

t("WorkersR2.putStream: a source that fails fails the write, with its own error", async () => {
  const r2 = fakeR2();
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  async function* broken() {
    yield new Uint8Array([1]);
    throw new Error("upstream reset");
  }
  await assert.rejects(store.putStream("k", broken(), 2), /upstream reset/);
  assert.equal(r2.objects.size, 0);
});

t("WorkersR2.putStream: goes through a FixedLengthStream of the size where the platform has one", async () => {
  const lengths = [];
  globalThis.FixedLengthStream = class extends TransformStream {
    constructor(n) {
      super();
      lengths.push(n);
    }
  };
  try {
    const r2 = fakeR2();
    await new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES").putStream("k", chunks([7, 8]), 2);
    assert.deepEqual(lengths, [2]);
    assert.deepEqual([...r2.objects.get("k").body], [7, 8]);
  } finally {
    delete globalThis.FixedLengthStream;
  }
});

t("WorkersR2.putStreamWith: content type, metadata and conditions, as putWith", async () => {
  const r2 = fakeR2();
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  const made = await store.putStreamWith("k", chunks([1]), 1, putOptions({ contentType: Some("text/plain"), metadata: new Map([["a", "b"]]), onlyIf: { tag: "Absent" } }));
  assert.equal(made.tag, "Some");
  assert.deepEqual(made.value.contentType, Some("text/plain"));
  assert.equal(made.value.metadata.get("a"), "b");
  assert.deepEqual(await store.putStreamWith("k", chunks([2]), 1, putOptions({ onlyIf: { tag: "Absent" } })), None);
  assert.deepEqual([...r2.objects.get("k").body], [1]);
  const swapped = await store.putStreamWith("k", chunks([3]), 1, putOptions({ onlyIf: { tag: "EtagMatches", etag: made.value.etag } }));
  assert.equal(swapped.tag, "Some");
  await assert.rejects(store.putStreamWith("k", chunks([1, 2]), 1, putOptions()), /putStreamWith: the stream held at least 2 bytes/);
});

t("WorkersR2.putStreamWith: a put that answers without reading the stream releases its source", async () => {
  // Real R2 may refuse a failed condition, or a put, before reading the body.
  for (const answer of [async () => null, async () => { throw new Error("binding down"); }]) {
    let released = false;
    // Two chunks: the stream reads ahead one, leaving the source open.
    async function* source() {
      try {
        yield new Uint8Array([1]);
        yield new Uint8Array([2]);
      } finally {
        released = true;
      }
    }
    const store = new WorkersR2({ R2_ARCHIVES: { put: answer } }, "R2_ARCHIVES");
    await store.putStreamWith("k", source(), 2, putOptions({ onlyIf: { tag: "Absent" } })).catch(() => {});
    await new Promise((r) => setTimeout(r, 0));
    assert.equal(released, true);
  }
});

t("WorkersR2.createUpload: begins an upload of the key, with the object's content type and metadata", async () => {
  const r2 = fakeR2();
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  const u = await store.createUpload("big", { contentType: Some("video/mp4"), metadata: new Map([["by", "me"]]) });
  assert.equal(u.key, "big");
  assert.equal(typeof u.uploadId, "string");
  assert.deepEqual(r2.uploads.get(u.uploadId).options, { httpMetadata: { contentType: "video/mp4" }, customMetadata: { by: "me" } });
});

t("WorkersR2.uploadPart: uploads a part of a resumed upload, reporting its number and etag", async () => {
  const r2 = fakeR2();
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  const u = await store.createUpload("big", { contentType: None, metadata: noMetadata() });
  // A fresh binding, as a later request would have: only the handle carries over.
  const part = await new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES").uploadPart({ key: u.key, uploadId: u.uploadId }, 2, bytes(9));
  assert.equal(part.partNumber, 2);
  assert.equal(typeof part.etag, "string");
  await assert.rejects(store.uploadPart({ key: "big", uploadId: "nope" }, 1, bytes(1)), /does not exist/);
});

t("WorkersR2.completeUpload: joins the parts in number order into the stored object", async () => {
  const r2 = fakeR2();
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  const u = await store.createUpload("big", { contentType: Some("text/plain"), metadata: noMetadata() });
  const p2 = await store.uploadPart(u, 2, bytes(3));
  const p1 = await store.uploadPart(u, 1, bytes(1, 2));
  const o = await store.completeUpload(u, [p2, p1]);
  assert.deepEqual(r2.completions, [[1, 2]]);
  assert.equal(o.key, "big");
  assert.equal(o.size, 3);
  assert.deepEqual(o.contentType, Some("text/plain"));
  assert.deepEqual([...r2.objects.get("big").body], [1, 2, 3]);
  await assert.rejects(store.completeUpload(u, [p1, p2]), /does not exist/);
});

t("WorkersR2.abortUpload: discards the upload and its parts", async () => {
  const r2 = fakeR2();
  const store = new WorkersR2({ R2_ARCHIVES: r2 }, "R2_ARCHIVES");
  const u = await store.createUpload("big", { contentType: None, metadata: noMetadata() });
  const p = await store.uploadPart(u, 1, bytes(1));
  await store.abortUpload(u);
  assert.equal(r2.uploads.size, 0);
  await assert.rejects(store.completeUpload(u, [p]), /does not exist/);
  assert.equal(r2.objects.has("big"), false);
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

// P2 (#2578): `WorkersQueue`, against a fake of Cloudflare's queue producer:
// `send(body, { contentType, delaySeconds })` and `sendBatch(messages)`, each
// message `{ body, contentType, delaySeconds }`.
function fakeQueue() {
  const sends = [];
  const batches = [];
  return {
    sends,
    batches,
    async send(body, options) {
      sends.push({ body, options });
    },
    async sendBatch(messages) {
      batches.push([...messages]);
    },
  };
}

t("WorkersQueue.send: sends the JSON text parsed, as contentType json", async () => {
  const q = fakeQueue();
  await new WorkersQueue({ QUEUE_ORDERS: q }, "QUEUE_ORDERS").send('{"id":7,"tags":["a"]}');
  assert.deepEqual(q.sends, [{ body: { id: 7, tags: ["a"] }, options: { contentType: "json" } }]);
});

t("WorkersQueue.send: text that is not JSON faults, and nothing is sent", async () => {
  const q = fakeQueue();
  await assert.rejects(
    new WorkersQueue({ QUEUE_ORDERS: q }, "QUEUE_ORDERS").send("not json"),
    /sends JSON text .* this body is not JSON/,
  );
  assert.equal(q.sends.length, 0);
});

t("WorkersQueue.send: a missing binding names it", async () => {
  await assert.rejects(
    new WorkersQueue({}, "QUEUE_ORDERS").send("1"),
    /requires a queue producer binding \(env\.QUEUE_ORDERS\)/,
  );
});

t("WorkersQueue.sendValue: puts on the wire exactly the message send(Json.encode(v)) does", async () => {
  // A value as the call site's `__serialise_X` leaves it: a `None`, a `()` as
  // null, a `-0.0`, a field that is absent (undefined), nested arrays.
  const encoded = { id: 7, note: { kind: "None" }, unit: null, f: -0, gone: undefined, tags: [["a"], []] };
  const viaSend = fakeQueue();
  await new WorkersQueue({ QUEUE_ORDERS: viaSend }, "QUEUE_ORDERS").send(JSON.stringify(encoded));
  const viaValue = fakeQueue();
  await new WorkersQueue({ QUEUE_ORDERS: viaValue }, "QUEUE_ORDERS").sendValue(encoded);
  assert.deepStrictEqual(viaValue.sends, viaSend.sends);
  assert.deepStrictEqual(viaValue.sends[0].options, { contentType: "json" });
  for (const scalar of [0, "s", true, null, [1, 2]]) {
    const a = fakeQueue();
    const b = fakeQueue();
    await new WorkersQueue({ QUEUE_ORDERS: a }, "QUEUE_ORDERS").send(JSON.stringify(scalar));
    await new WorkersQueue({ QUEUE_ORDERS: b }, "QUEUE_ORDERS").sendValue(scalar);
    assert.deepStrictEqual(b.sends, a.sends);
  }
});

t("WorkersQueue.sendValue: a missing binding names it", async () => {
  await assert.rejects(
    new WorkersQueue({}, "QUEUE_ORDERS").sendValue(1),
    /requires a queue producer binding \(env\.QUEUE_ORDERS\)/,
  );
});

t("WorkersQueue.sendWith: a delay goes as whole seconds, a part second rounded up", async () => {
  const q = fakeQueue();
  const queue = new WorkersQueue({ QUEUE_ORDERS: q }, "QUEUE_ORDERS");
  await queue.sendWith('"a"', { delay: Some(90_000) });
  await queue.sendWith('"b"', { delay: Some(1_500) });
  await queue.sendWith('"c"', { delay: None });
  assert.deepEqual(q.sends, [
    { body: "a", options: { contentType: "json", delaySeconds: 90 } },
    { body: "b", options: { contentType: "json", delaySeconds: 2 } },
    { body: "c", options: { contentType: "json" } },
  ]);
});

t("WorkersQueue.sendWith: a delay over 12 hours or below zero faults", async () => {
  const q = fakeQueue();
  const queue = new WorkersQueue({ QUEUE_ORDERS: q }, "QUEUE_ORDERS");
  await assert.rejects(queue.sendWith("1", { delay: Some(43_201_000) }), /between 0 seconds and 12 hours/);
  await assert.rejects(queue.sendWith("1", { delay: Some(-1_000) }), /between 0 seconds and 12 hours/);
  await assert.rejects(queue.sendWith("1", { delay: Some(-500) }), /between 0 seconds and 12 hours/);
  assert.equal(q.sends.length, 0);
});

t("WorkersQueue.sendBatch: one call, every body parsed; all or none", async () => {
  const q = fakeQueue();
  const queue = new WorkersQueue({ QUEUE_ORDERS: q }, "QUEUE_ORDERS");
  await queue.sendBatch(["1", '{"k":"v"}']);
  assert.deepEqual(q.batches, [
    [
      { body: 1, contentType: "json" },
      { body: { k: "v" }, contentType: "json" },
    ],
  ]);
  await assert.rejects(queue.sendBatch(["2", "nope"]), /this body is not JSON/);
  await queue.sendBatch([]);
  assert.equal(q.batches.length, 1);
});

// L7 (#2414): `WorkersEdgeCache`, against a fake of the Workers
// `ExecutionContext.cache`: `purge({ pathPrefixes })` resolves `{ success,
// errors }`, `success: false` when Cloudflare refuses (its rate limit).
function fakeCtx(success = true) {
  const calls = [];
  return {
    calls,
    cache: {
      async purge(options) {
        calls.push(options);
        return { success, errors: success ? [] : [{ code: 429, message: "rate limited" }] };
      },
    },
  };
}

t("WorkersEdgeCache.purge: purges the prefixes through ctx.cache", async () => {
  const ctx = fakeCtx();
  assert.equal(await new WorkersEdgeCache(ctx).purge(["/dl/sha256/abc", "/index/"]), true);
  assert.deepEqual(ctx.calls, [{ pathPrefixes: ["/dl/sha256/abc", "/index/"] }]);
});

t("WorkersEdgeCache.purge: a refused purge is false", async () => {
  assert.equal(await new WorkersEdgeCache(fakeCtx(false)).purge(["/a"]), false);
});

t("WorkersEdgeCache.purge: an empty list purges nothing and is true", async () => {
  const ctx = fakeCtx(false);
  assert.equal(await new WorkersEdgeCache(ctx).purge([]), true);
  assert.equal(ctx.calls.length, 0);
});

t("WorkersEdgeCache.purge: a prefix that is not a plain path faults", async () => {
  const ctx = fakeCtx();
  for (const bad of ["dl/x", "https://x/y", "/a?b", "/a#b", "/a\nb", ""]) {
    await assert.rejects(new WorkersEdgeCache(ctx).purge(["/ok", bad]), /takes paths starting with `\/`/, bad);
  }
  assert.equal(ctx.calls.length, 0);
});

t("WorkersEdgeCache.purge: with no ctx.cache it logs and is true", async () => {
  const logged = [];
  const real = console.log;
  console.log = (...args) => logged.push(args.join(" "));
  try {
    assert.equal(await new WorkersEdgeCache(undefined).purge(["/a"]), true);
    assert.equal(await new WorkersEdgeCache({ waitUntil() {} }).purge(["/a"]), true);
  } finally {
    console.log = real;
  }
  assert.deepEqual(logged, [
    "bynk: EdgeCache.purge is a no-op under bynk dev",
    "bynk: EdgeCache.purge is a no-op under bynk dev",
  ]);
});

// P3b (#2582): `WorkersAnalytics`, against a fake of Cloudflare's Analytics
// Engine dataset: a synchronous `writeDataPoint({ indexes, blobs, doubles })`.
function fakeDataset() {
  const points = [];
  return {
    points,
    writeDataPoint(point) {
      points.push(point);
    },
  };
}

const analytics = (d) => new WorkersAnalytics({ ANALYTICS_SIGNUPS: d }, "ANALYTICS_SIGNUPS");

t("WorkersAnalytics.write: writes the index, blobs and doubles in order", async () => {
  const d = fakeDataset();
  await analytics(d).write({ index: Some("eu"), blobs: ["pro", "web"], doubles: [1, 2.5] });
  await analytics(d).write({ index: None, blobs: [], doubles: [] });
  assert.deepEqual(d.points, [
    { indexes: ["eu"], blobs: ["pro", "web"], doubles: [1, 2.5] },
    { indexes: [], blobs: [], doubles: [] },
  ]);
});

t("WorkersAnalytics.write: a point over the platform's limits faults, and nothing is written", async () => {
  const d = fakeDataset();
  const point = (p) => ({ index: None, blobs: [], doubles: [], ...p });
  await assert.rejects(analytics(d).write(point({ blobs: Array(21).fill("x") })), /at most 20 blobs, not 21/);
  await assert.rejects(analytics(d).write(point({ doubles: Array(21).fill(0) })), /at most 20 doubles, not 21/);
  // Bytes, not characters: 49 two-byte characters are 98 bytes.
  await assert.rejects(analytics(d).write(point({ index: Some("é".repeat(49)) })), /at most 96 bytes, not 98/);
  await assert.rejects(
    analytics(d).write(point({ blobs: ["a".repeat(10_000), "b".repeat(6_385)] })),
    /total at most 16 KB, not 16385 bytes/,
  );
  await assert.rejects(analytics(d).write(point({ doubles: [Number.NaN] })), /finite/);
  await assert.rejects(analytics(d).write(point({ doubles: [Infinity] })), /finite/);
  assert.equal(d.points.length, 0);
  // At the limits exactly, the write goes through.
  await analytics(d).write(
    point({ index: Some("a".repeat(96)), blobs: ["a".repeat(16_384)], doubles: Array(20).fill(0) }),
  );
  assert.equal(d.points.length, 1);
});

t("WorkersAnalytics.write: a missing binding names it", async () => {
  await assert.rejects(
    new WorkersAnalytics({}, "ANALYTICS_SIGNUPS").write({ index: None, blobs: [], doubles: [] }),
    /requires an Analytics Engine binding \(env\.ANALYTICS_SIGNUPS\)/,
  );
});

// P1 (#2590): the new `Kv` operations.
const never = { tag: "Never" };
const putOpts = (o = {}) => ({ expiration: never, metadata: new Map(), ...o });

t("WorkersKv.putWith: passes a relative or absolute expiry in seconds, and the metadata", async () => {
  const kv = fakeKv();
  const store = new WorkersKv({ KV: kv });
  await store.putWith("a", "1", putOpts({ expiration: { tag: "After", ttl: 90_500 }, metadata: new Map([["k", "v"]]) }));
  const at = Date.now() + 3_600_000;
  await store.putWith("b", "2", putOpts({ expiration: { tag: "At", instant: at } }));
  await store.putWith("c", "3", putOpts());
  assert.deepEqual(kv.puts.map((p) => p.options), [
    { expirationTtl: 91, metadata: { k: "v" } },
    { expiration: Math.ceil(at / 1000), metadata: {} },
    { metadata: {} },
  ]);
});

t("WorkersKv.putWith: a near expiry, oversized metadata or a long key faults, and nothing is written", async () => {
  const kv = fakeKv();
  const store = new WorkersKv({ KV: kv });
  await assert.rejects(store.putWith("a", "1", putOpts({ expiration: { tag: "After", ttl: 59_000 } })), /at least 60 seconds/);
  await assert.rejects(store.putWith("a", "1", putOpts({ expiration: { tag: "At", instant: Date.now() + 10_000 } })), /at least 60 seconds/);
  await assert.rejects(store.putWith("a", "1", putOpts({ metadata: new Map([["k", "x".repeat(1100)]]) })), /at most 1024 bytes as JSON/);
  await assert.rejects(store.putWith("é".repeat(257), "1", putOpts()), /at most 512 bytes, not 514/);
  assert.equal(kv.puts.length, 0);
  // Exactly at the limits, the write goes through.
  await store.putWith("k".repeat(512), "1", putOpts({ expiration: { tag: "After", ttl: 60_000 } }));
  assert.equal(kv.puts.length, 1);
});

t("WorkersKv.getWith: a value with its metadata, edge-cached for cacheTtl", async () => {
  const kv = fakeKv();
  const store = new WorkersKv({ KV: kv });
  assert.deepEqual(await store.getWith("absent", { cacheTtl: None }), None);
  await store.putWith("a", "1", putOpts({ metadata: new Map([["k", "v"]]) }));
  kv.entries.set("b", "2");
  kv.meta.set("b", { n: 3, ok: true });
  const a = await store.getWith("a", { cacheTtl: Some(45_000) });
  assert.equal(a.tag, "Some");
  assert.equal(a.value.value, "1");
  assert.deepEqual([...a.value.metadata], [["k", "v"]]);
  // Metadata written outside Bynk may hold any JSON: a non-string is its text.
  const b = await store.getWith("b", { cacheTtl: None });
  assert.deepEqual([...b.value.metadata], [["n", "3"], ["ok", "true"]]);
  assert.deepEqual(kv.reads.filter((r) => r.options).map((r) => r.options), [
    { type: "text" },
    { type: "text", cacheTtl: 45 },
    { type: "text" },
  ]);
  await assert.rejects(store.getWith("a", { cacheTtl: Some(29_000) }), /at least 30 seconds/);
});

t("WorkersKv.getBytes: a binary value as bytes, and a text value as its UTF-8", async () => {
  const kv = fakeKv();
  const store = new WorkersKv({ KV: kv });
  assert.deepEqual(await store.getBytes("absent"), None);
  kv.entries.set("text", "é");
  const r = await store.getBytes("text");
  assert.deepEqual([...r.value], [0xc3, 0xa9]);
});

t("WorkersKv.putBytes: writes the bytes with the options, readable as text when UTF-8", async () => {
  const kv = fakeKv();
  const store = new WorkersKv({ KV: kv });
  await store.putBytes("b", new Uint8Array([0, 255, 7]), putOpts({ metadata: new Map([["kind", "blob"]]) }));
  assert.deepEqual([...(await store.getBytes("b")).value], [0, 255, 7]);
  assert.deepEqual(kv.puts[0].options, { metadata: { kind: "blob" } });
  await store.putBytes("t", new TextEncoder().encode("hi"), putOpts());
  assert.deepEqual(await store.get("t"), Some("hi"));
});

t("WorkersKv.listPage: one page at a time, with each key's expiry and metadata, to a None cursor", async () => {
  const kv = fakeKv(2);
  const store = new WorkersKv({ KV: kv });
  for (const k of ["a", "b", "c"]) await store.put(k, k);
  kv.meta.set("a", { k: "v" });
  await store.putWith("b", "b", putOpts({ expiration: { tag: "At", instant: 4_000_000_000_000 } }));
  const first = await store.listPage({ prefix: None, cursor: None, limit: Some(2) });
  assert.deepEqual(first.keys.map((k) => k.name), ["a", "b"]);
  assert.deepEqual([...first.keys[0].metadata], [["k", "v"]]);
  assert.deepEqual(first.keys[0].expiration, None);
  assert.deepEqual(first.keys[1].expiration, Some(4_000_000_000_000));
  assert.equal(first.cursor.tag, "Some");
  const second = await store.listPage({ prefix: None, cursor: first.cursor, limit: Some(2) });
  assert.deepEqual(second.keys.map((k) => k.name), ["c"]);
  assert.deepEqual(second.cursor, None);
  for (const bad of [0, 1001, 1.5]) {
    await assert.rejects(store.listPage({ prefix: None, cursor: None, limit: Some(bad) }), /1 to 1000 keys/);
  }
});

// P4a (#2602): `WorkersD1`, against a fake of Cloudflare's D1 binding:
// `prepare(sql).bind(...values)` and a statement's `all`/`first`/`run`, each
// answering what the fake was given, and `batch(statements)`. It records each
// statement's SQL and bound values; `fail` makes every call reject with it.
function fakeD1({ rows = [], meta = { changes: 0, last_row_id: 0 }, fail } = {}) {
  const seen = [];
  const batches = [];
  const statement = (sql, values) => ({
    sql,
    values,
    bind: (...v) => statement(sql, v),
    async all() {
      seen.push({ sql, values });
      if (fail) throw fail;
      return { results: rows, meta };
    },
    async first() {
      seen.push({ sql, values });
      if (fail) throw fail;
      return rows[0] ?? null;
    },
    async run() {
      seen.push({ sql, values });
      if (fail) throw fail;
      return { results: [], meta };
    },
  });
  return {
    seen,
    batches,
    prepare: (sql) => statement(sql, []),
    async batch(statements) {
      batches.push(statements.map((s) => ({ sql: s.sql, values: s.values })));
      if (fail) throw fail;
      return statements.map(() => ({ results: [], meta }));
    },
  };
}

const d1 = (db) => new WorkersD1({ D1_ORDERS: db }, "D1_ORDERS");
const text = (value) => ({ tag: "Text", value });

t("WorkersD1: binds each SqlValue positionally, a Boolean as 1 or 0", async () => {
  const db = fakeD1();
  const bytes = new Uint8Array([1, 2]);
  await d1(db).run("INSERT INTO t VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", [
    { tag: "Null" },
    { tag: "Integer", value: 7 },
    { tag: "Real", value: 1.5 },
    text("a"),
    { tag: "Blob", value: bytes },
    { tag: "Boolean", value: true },
    { tag: "Boolean", value: false },
  ]);
  assert.deepEqual(db.seen, [
    { sql: "INSERT INTO t VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", values: [null, 7, 1.5, "a", bytes, 1, 0] },
  ]);
});

t("WorkersD1.all: the rows as JSON, each blob as standard base64 whatever its shape", async () => {
  const ab = new Uint8Array([0xfb, 0xff]).buffer;
  const db = fakeD1({
    rows: [
      { id: "a", n: 1, f: 0.5, none: null, ab, view: new Uint8Array([104, 105]), arr: [0, 255] },
    ],
  });
  const r = await d1(db).all("SELECT * FROM t", []);
  assert.equal(r.tag, "Ok");
  assert.deepEqual(JSON.parse(r.value), [
    { id: "a", n: 1, f: 0.5, none: null, ab: "+/8=", view: "aGk=", arr: "AP8=" },
  ]);
  assert.deepEqual(await d1(fakeD1()).all("SELECT 1 WHERE 0", []), { tag: "Ok", value: "[]" });
});

t("WorkersD1.first: one row as JSON, or None", async () => {
  const one = await d1(fakeD1({ rows: [{ id: "a" }, { id: "b" }] })).first("SELECT id FROM t", []);
  assert.deepEqual(one, { tag: "Ok", value: Some(JSON.stringify({ id: "a" })) });
  assert.deepEqual(await d1(fakeD1()).first("SELECT id FROM t", []), { tag: "Ok", value: None });
});

t("WorkersD1.run: what the write changed", async () => {
  const r = await d1(fakeD1({ meta: { changes: 3, last_row_id: 9 } })).run("DELETE FROM t", []);
  assert.deepEqual(r, { tag: "Ok", value: { changes: 3, lastRowId: 9 } });
});

t("WorkersD1: a constraint failure is Constraint, any other failure Failed, never a fault", async () => {
  const unique = new Error("D1_ERROR: UNIQUE constraint failed: t.id: SQLITE_CONSTRAINT");
  const syntax = new Error('D1_ERROR: near "SELEC": syntax error at offset 0: SQLITE_ERROR');
  assert.deepEqual(await d1(fakeD1({ fail: unique })).run("INSERT …", []), {
    tag: "Err",
    error: { tag: "Constraint", message: unique.message },
  });
  for (const op of [(d) => d.all("SELEC", []), (d) => d.first("SELEC", []), (d) => d.run("SELEC", [])]) {
    assert.deepEqual(await op(d1(fakeD1({ fail: syntax }))), {
      tag: "Err",
      error: { tag: "Failed", message: syntax.message },
    });
  }
});

t("WorkersD1.batch: one platform batch, a run per statement, or the failure", async () => {
  const db = fakeD1({ meta: { changes: 1, last_row_id: 2 } });
  const r = await d1(db).batch([
    { sql: "UPDATE t SET n = n - 1 WHERE id = ?1", params: [text("a")] },
    { sql: "UPDATE t SET n = n + 1 WHERE id = ?1", params: [text("b")] },
  ]);
  assert.deepEqual(r, { tag: "Ok", value: [{ changes: 1, lastRowId: 2 }, { changes: 1, lastRowId: 2 }] });
  assert.deepEqual(db.batches, [
    [
      { sql: "UPDATE t SET n = n - 1 WHERE id = ?1", values: ["a"] },
      { sql: "UPDATE t SET n = n + 1 WHERE id = ?1", values: ["b"] },
    ],
  ]);
  // D1 refuses an empty batch; an empty one is nothing to do.
  assert.deepEqual(await d1(db).batch([]), { tag: "Ok", value: [] });
  assert.equal(db.batches.length, 1);
  const failing = fakeD1({ fail: new Error("D1_ERROR: CHECK constraint failed: n >= 0: SQLITE_CONSTRAINT") });
  assert.equal((await d1(failing).batch([{ sql: "UPDATE t", params: [] }])).error.tag, "Constraint");
});

t("WorkersD1: a missing binding names it", async () => {
  await assert.rejects(
    new WorkersD1({}, "D1_ORDERS").all("SELECT 1", []),
    /requires a D1 database binding \(env\.D1_ORDERS\)/,
  );
});
