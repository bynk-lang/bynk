// `cloudflare.binding`: the `bynk.cloudflare` adapter's `WorkersKv` binding,
// tested against a KV namespace fake (standard-library S2, #1952), its
// `WorkersService` (interop I5, #1972) against a Service Binding fake, and its
// `WorkersR2` (standard-library P5, #2323; streams and multipart, P5b, #2626)
// against an R2 bucket fake, and its
// `WorkersRateLimit` (standard-library P3, #2342) against a rate limiter fake,
// `WorkersQueue` (standard-library P2, #2578) against a queue producer fake,
// `WorkersAnalytics` (standard-library P3b, #2582) against a dataset fake,
// `WorkersD1` (standard-library P4a, #2602) against a database fake,
// `WorkersEdgeCache` (L7, #2414) against an execution context fake, and
// `WorkersVersion` (#3300) against a Version metadata fake, and
// `WorkersAi` (#3478) against a Workers AI binding fake, and
// `WorkersVectorize` (#3479) against a Vectorize index fake, and
// `WorkersAiSearch` (#3480) against an AI Search instance fake, and
// `WorkersStoredSecret` (#3301) against a Secrets Store secret fake, and
// `WorkersEmail` (#3302) against a `send_email` binding fake, and
// `WorkersPostgres` (Hyperdrive H1, #3318) against a driver-executor fake (the
// driver seam itself, with the `prepare: true` guard, is tested in the runtime),
// and `WorkersMySql` (Hyperdrive H5, #3322) against the same kind of fake.
//
// The fake follows Cloudflare's documented `KVNamespace` contract, not the
// binding: `get` resolves `null` for an absent key; `list` returns keys in
// lexicographic order, at most `limit` per page (1000 by default), with
// `list_complete: false` and a `cursor` while more remain, and no cursor on
// the last page; `put` takes `{ expirationTtl }` in seconds. Pages are small
// here so that draining several of them is exercised. Whether the fake matches
// the platform is what `bynk-tests/tests/binding_workerd.rs` checks, on `workerd`.

import { test } from "node:test";
import assert from "node:assert/strict";
import { WorkersAi, WorkersAiSearch, WorkersAnalytics, WorkersContainer, WorkersD1, WorkersEdgeCache, WorkersEmail, WorkersKv, WorkersMySql, WorkersPostgres, WorkersQueue, WorkersR2, WorkersRateLimit, WorkersService, WorkersStoredSecret, WorkersVectorize, WorkersVersion } from "./bynk/cloudflare.binding.js";
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
// `bynk-tests/tests/binding_workerd.rs` checks, on `workerd`.
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

t("WorkersQueue.sendValueWith: puts on the wire exactly what sendWith(Json.encode(v), options) does", async () => {
  const encoded = { id: 7, note: { kind: "None" }, f: -0, gone: undefined, tags: [["a"], []] };
  for (const options of [{ delay: Some(90_000) }, { delay: Some(1_500) }, { delay: None }]) {
    const viaWith = fakeQueue();
    await new WorkersQueue({ QUEUE_ORDERS: viaWith }, "QUEUE_ORDERS").sendWith(JSON.stringify(encoded), options);
    const viaValue = fakeQueue();
    await new WorkersQueue({ QUEUE_ORDERS: viaValue }, "QUEUE_ORDERS").sendValueWith(encoded, options);
    assert.deepStrictEqual(viaValue.sends, viaWith.sends);
  }
});

t("WorkersQueue.sendValueWith: a delay over 12 hours or below zero faults, and a missing binding names it", async () => {
  const q = fakeQueue();
  const queue = new WorkersQueue({ QUEUE_ORDERS: q }, "QUEUE_ORDERS");
  await assert.rejects(queue.sendValueWith(1, { delay: Some(43_201_000) }), /between 0 seconds and 12 hours/);
  await assert.rejects(queue.sendValueWith(1, { delay: Some(-500) }), /between 0 seconds and 12 hours/);
  assert.equal(q.sends.length, 0);
  await assert.rejects(
    new WorkersQueue({}, "QUEUE_ORDERS").sendValueWith(1, { delay: None }),
    /requires a queue producer binding \(env\.QUEUE_ORDERS\)/,
  );
});

t("WorkersQueue.sendValueBatch: puts on the wire exactly what sendBatch(xs.map(Json.encode)) does", async () => {
  const items = [{ id: 1, note: { kind: "None" }, f: -0, gone: undefined }, { id: 2, tags: [[], ["x"]] }];
  const viaBatch = fakeQueue();
  await new WorkersQueue({ QUEUE_ORDERS: viaBatch }, "QUEUE_ORDERS").sendBatch(items.map((m) => JSON.stringify(m)));
  const viaValue = fakeQueue();
  await new WorkersQueue({ QUEUE_ORDERS: viaValue }, "QUEUE_ORDERS").sendValueBatch(items);
  assert.deepStrictEqual(viaValue.batches, viaBatch.batches);
  assert.equal(viaValue.batches.length, 1);
});

t("WorkersQueue.sendValueBatch: an empty list sends nothing, and a missing binding names it", async () => {
  const q = fakeQueue();
  await new WorkersQueue({ QUEUE_ORDERS: q }, "QUEUE_ORDERS").sendValueBatch([]);
  assert.equal(q.batches.length, 0);
  await assert.rejects(
    new WorkersQueue({}, "QUEUE_ORDERS").sendValueBatch([1]),
    /requires a queue producer binding \(env\.QUEUE_ORDERS\)/,
  );
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

// #3300: `WorkersVersion`, against a fake of Cloudflare's Version metadata
// binding: `{ id, tag, timestamp }`, the timestamp an ISO-8601 string and the
// tag empty when the version has none.
t("WorkersVersion.current: reads the bare binding, the tag as an option and the timestamp as epoch milliseconds", async () => {
  const env = { CF_VERSION_METADATA: { id: "a1b2", tag: "v1.4.0", timestamp: "2026-11-11T09:30:00.000Z" } };
  assert.deepEqual(await new WorkersVersion(env).current(), {
    id: "a1b2",
    tag: Some("v1.4.0"),
    deployed: Date.UTC(2026, 10, 11, 9, 30),
  });
  const untagged = { CF_VERSION_METADATA: { id: "c3d4", tag: "", timestamp: "1970-01-01T00:00:00.000Z" } };
  assert.deepEqual(await new WorkersVersion(untagged).current(), { id: "c3d4", tag: None, deployed: 0 });
});

t("WorkersVersion.current: a missing binding or an unreadable timestamp faults", async () => {
  await assert.rejects(new WorkersVersion({}).current(), /\[version_metadata\]/);
  const bad = { CF_VERSION_METADATA: { id: "x", tag: "", timestamp: "yesterday" } };
  await assert.rejects(new WorkersVersion(bad).current(), /not a date/);
});

// #3478: `WorkersAi`, against a fake of Cloudflare's AI binding: `run(model,
// input)` resolves the model's output, or rejects with an error whose message
// carries the platform's internal code (the Workers AI errors page gives the
// codes, not the thrown object's fields, so both a numeric `code` and a code in
// the message are read).
function fakeAi(reply) {
  const calls = [];
  return {
    calls,
    async run(model, input) {
      calls.push({ model, input });
      if (reply instanceof Error) throw reply;
      if (reply?.throws !== undefined) throw reply.throws;
      return reply;
    },
  };
}

t("WorkersAi.run: passes the model and the parsed input, and returns the reply as JSON text", async () => {
  const ai = fakeAi({ response: "bonjour" });
  const r = await new WorkersAi({ AI: ai }).run("@cf/meta/llama-3.2-1b-instruct", '{"prompt":"hi","max_tokens":8}');
  assert.deepEqual(r, { tag: "Ok", value: '{"response":"bonjour"}' });
  assert.deepEqual(ai.calls, [{ model: "@cf/meta/llama-3.2-1b-instruct", input: { prompt: "hi", max_tokens: 8 } }]);
});

t("WorkersAi.run: a documented platform code is a typed error, any other a failure with its message", async () => {
  const failure = async (e) => (await new WorkersAi({ AI: fakeAi(e) }).run("@cf/x/y", "{}")).error;
  assert.deepEqual(await failure(new Error("5007: No such model `@cf/x/y` or task")), { tag: "ModelNotFound" });
  assert.deepEqual(await failure(new Error("3042: The model name is invalid")), { tag: "ModelNotFound" });
  assert.deepEqual(await failure(new Error("5028: `@cf/x/y` was deprecated")), { tag: "ModelNotFound" });
  assert.deepEqual(await failure(new Error("3036: used up")), { tag: "QuotaExceeded" });
  assert.deepEqual(await failure(new Error("5035: requires a plan")), { tag: "QuotaExceeded" });
  assert.deepEqual(await failure(new Error("3040: Capacity temporarily exceeded")), { tag: "Overloaded" });
  assert.deepEqual(await failure(new Error("AiError: 3040: Capacity temporarily exceeded")), { tag: "Overloaded" });
  assert.deepEqual(await failure(Object.assign(new Error("bad"), { code: 5004 })), { tag: "InvalidInput", message: "bad" });
  assert.deepEqual(await failure(Object.assign(new Error("bad"), { code: "3006" })), { tag: "InvalidInput", message: "bad" });
  assert.deepEqual(await failure(new Error("3007: Request timeout")), { tag: "AiFailed", message: "3007: Request timeout" });
  assert.deepEqual(await failure(new Error("something new")), { tag: "AiFailed", message: "something new" });
  assert.deepEqual(await failure({ throws: "odd" }), { tag: "AiFailed", message: "odd" });
  // Only a code that opens the message is one: a number elsewhere in it is not.
  assert.deepEqual(await failure(new Error("Model @cf/x/foo-3040 failed")), { tag: "AiFailed", message: "Model @cf/x/foo-3040 failed" });
  assert.deepEqual(await failure(new Error("input of 3003 tokens too long")), { tag: "AiFailed", message: "input of 3003 tokens too long" });
  // A number that merely sits in a longer one is not a code.
  assert.deepEqual(await failure(new Error("id 350071 failed")), { tag: "AiFailed", message: "id 350071 failed" });
});

// Observed on a real account (#3478, W39): the binding throws an `Error` named
// `AiError` (constructor `InferenceUpstreamError`) with no `code` property and a
// message that opens with the platform's code.
t("WorkersAi.run: the error a live binding throws is read from its message, 5006 included", async () => {
  class InferenceUpstreamError extends Error {
    constructor(message) {
      super(message);
      this.name = "AiError";
    }
  }
  const live = (message) => new InferenceUpstreamError(message);
  const failure = async (e) => (await new WorkersAi({ AI: fakeAi(e) }).run("@cf/x/y", "{}")).error;
  assert.equal("code" in live("5007: x"), false);
  assert.deepEqual(await failure(live("5007: No such model @cf/nobody/no-such-model or task")), { tag: "ModelNotFound" });
  assert.deepEqual(await failure(live("5028: @cf/x/y was deprecated on 2026-05-30. See the model catalog for alternatives: https://developers.cloudflare.com/workers-ai/models/")), { tag: "ModelNotFound" });
  const rejected = "5006: Error: oneOf at '/' not met, 0 matches: required properties at '/' are 'prompt', required properties at '/' are 'messages'";
  assert.deepEqual(await failure(live(rejected)), { tag: "InvalidInput", message: rejected });
});

t("WorkersAi.run: a live stream (input with stream: true) is cancelled and is a failure", async () => {
  let cancelled = false;
  const stream = new ReadableStream({ cancel() { cancelled = true; } });
  const r = await new WorkersAi({ AI: fakeAi(stream) }).run("@cf/x/y", '{"stream":true}');
  assert.equal(r.tag, "Err");
  assert.equal(cancelled, true);
});

t("WorkersAi.run: a binary reply is a failure, not a fault", async () => {
  for (const bytes of [new Uint8Array([1, 2]), new ArrayBuffer(2), new ReadableStream()]) {
    const r = await new WorkersAi({ AI: fakeAi(bytes) }).run("@cf/x/y", "{}");
    assert.deepEqual(r.error, { tag: "AiFailed", message: "model returned binary data; run supports JSON replies only" });
  }
});

t("WorkersAi.run: a bad model id, an input that is not a JSON object and a missing binding fault", async () => {
  const ai = new WorkersAi({ AI: fakeAi({}) });
  for (const model of ["", "has space", "a".repeat(129), "bad\u00e9"]) {
    await assert.rejects(ai.run(model, "{}"), /not a model id/);
  }
  for (const input of ["not json", "[1]", "3", "null", '"s"']) {
    await assert.rejects(ai.run("@cf/x/y", input), /`input`/);
  }
  await assert.rejects(new WorkersAi({}).run("@cf/x/y", "{}"), /\[ai\]/);
});

const chat = (role, content) => ({ role: { tag: role }, content });
const request = (extra = {}) => ({ messages: [chat("User", "hi")], maxTokens: None, temperature: None, topP: None, seed: None, ...extra });

t("WorkersAi.generate: sends the messages with lower-case roles and only the settings given, and reads text and usage", async () => {
  const ai = fakeAi({ response: "hello", usage: { prompt_tokens: 3, completion_tokens: 2, total_tokens: 5 } });
  const r = await new WorkersAi({ AI: ai }).generate(
    "@cf/meta/llama-3.2-1b-instruct",
    request({ messages: [chat("System", "be brief"), chat("User", "hi"), chat("Assistant", "hey")], maxTokens: Some(64), temperature: Some(0.5), topP: Some(0.9), seed: Some(7) }),
  );
  assert.deepEqual(r, { tag: "Ok", value: { text: "hello", usage: Some({ promptTokens: 3, completionTokens: 2 }) } });
  assert.deepEqual(ai.calls[0].input, {
    messages: [{ role: "system", content: "be brief" }, { role: "user", content: "hi" }, { role: "assistant", content: "hey" }],
    max_tokens: 64,
    temperature: 0.5,
    top_p: 0.9,
    seed: 7,
  });
  const plain = fakeAi({ response: "ok" });
  await new WorkersAi({ AI: plain }).generate("@cf/x/y", request());
  assert.deepEqual(plain.calls[0].input, { messages: [{ role: "user", content: "hi" }] });
});

t("WorkersAi.generate: a missing or odd usage is None, and a reply that is not text is a BadReply", async () => {
  const gen = async (reply) => new WorkersAi({ AI: fakeAi(reply) }).generate("@cf/x/y", request());
  assert.deepEqual((await gen({ response: "t" })).value, { text: "t", usage: None });
  assert.deepEqual((await gen({ response: "t", usage: { prompt_tokens: "3" } })).value.usage, None);
  assert.equal((await gen({ response: "" })).value.text, "");
  for (const bad of [{}, { response: 5 }, { response: null, tool_calls: [] }, { response: { a: 1 } }, "text", null, [1], new Uint8Array(2)]) {
    const r = await gen(bad);
    assert.equal(r.error?.tag, "BadReply", JSON.stringify(bad));
  }
  let cancelled = false;
  const r = await gen(new ReadableStream({ cancel() { cancelled = true; } }));
  assert.equal(r.error.tag, "BadReply");
  assert.equal(cancelled, true);
});

// Shapes observed on a real account (#3478, W39) for a `messages` request.
t("WorkersAi.generate: reads `choices[0].message.content` when there is no `response`, and prefers `response` when both are there", async () => {
  const gen = async (reply) => new WorkersAi({ AI: fakeAi(reply) }).generate("@cf/x/y", request());
  const usage = { prompt_tokens: 36, completion_tokens: 3, total_tokens: 39, prompt_tokens_details: { cached_tokens: 0 }, neurons: 0.09 };
  // granite 4.0 micro: OpenAI-style only.
  const granite = { id: "chatcmpl-1", object: "chat.completion", model: "@cf/ibm-granite/granite-4.0-h-micro", choices: [{ index: 0, message: { role: "assistant", content: "Hello!", refusal: null }, finish_reason: "stop" }], usage };
  assert.deepEqual((await gen(granite)).value, { text: "Hello!", usage: Some({ promptTokens: 36, completionTokens: 3 }) });
  // llama 3.1 8b fast: both; `response` is read.
  const both = { ...granite, choices: [{ index: 0, message: { role: "assistant", content: "from choices" } }], response: "from response" };
  assert.equal((await gen(both)).value.text, "from response");
  // llama 3.2 1b: `response` only.
  assert.equal((await gen({ response: "Hi", usage })).value.text, "Hi");
  assert.deepEqual((await gen({ choices: [{ message: { content: "" } }] })).value, { text: "", usage: None });
  // A `response` that is there but not text does not block the fallback.
  assert.equal((await gen({ response: null, choices: [{ message: { content: "x" } }] })).value.text, "x");
  // Neither shape is a BadReply: no content, or content that is not text.
  for (const bad of [{ choices: [] }, { choices: [{}] }, { choices: [{ message: {} }] }, { choices: [{ message: { content: null } }] }, { choices: [{ message: { content: 5 } }] }, { choices: "x" }, { choices: [{ text: "completion shape" }] }]) {
    assert.equal((await gen(bad)).error?.tag, "BadReply", JSON.stringify(bad));
  }
});

t("WorkersAi.generate: a platform refusal is a typed error, and empty messages or a bad model fault", async () => {
  const ai = (e) => new WorkersAi({ AI: fakeAi(e) });
  assert.deepEqual((await ai(new Error("5007: No such model")).generate("@cf/x/y", request())).error, { tag: "ModelNotFound" });
  assert.deepEqual((await ai(new Error("3040: busy")).generate("@cf/x/y", request())).error, { tag: "Overloaded" });
  await assert.rejects(ai({}).generate("@cf/x/y", request({ messages: [] })), /at least one message/);
  await assert.rejects(ai({}).generate("", request()), /not a model id/);
  // A role this binding has no wire name for is refused, not sent as a guess.
  await assert.rejects(ai({}).generate("@cf/x/y", request({ messages: [chat("ToolCall", "x")] })), /no wire name/);
});

t("WorkersAi.embed: sends the texts as a list and returns one vector per text in order", async () => {
  const ai = fakeAi({ shape: [2, 3], data: [[0.1, 0.2, 0.3], [1, 2, 3]], pooling: "mean" });
  const r = await new WorkersAi({ AI: ai }).embed("@cf/baai/bge-base-en-v1.5", ["a", "b"]);
  assert.deepEqual(r, { tag: "Ok", value: [[0.1, 0.2, 0.3], [1, 2, 3]] });
  assert.deepEqual(ai.calls, [{ model: "@cf/baai/bge-base-en-v1.5", input: { text: ["a", "b"] } }]);
  // No `shape` in the reply is fine: it is optional.
  assert.equal((await new WorkersAi({ AI: fakeAi({ data: [[1, 2], [3, 4]] }) }).embed("@cf/x/y", ["a", "b"])).tag, "Ok");
  // One text is still a list, so the reply is always a list of vectors.
  const one = fakeAi({ data: [[1]] });
  await new WorkersAi({ AI: one }).embed("@cf/x/y", ["only"]);
  assert.deepEqual(one.calls[0].input, { text: ["only"] });
});

t("WorkersAi.embed: a reply that is not one list of numbers per text is a BadReply, a refusal a typed error, and an empty batch a fault", async () => {
  const emb = async (reply, texts = ["a", "b"]) => new WorkersAi({ AI: fakeAi(reply) }).embed("@cf/x/y", texts);
  for (const bad of [{}, { data: "x" }, { data: [1, 2] }, { data: [[1], ["a"]] }, { data: [[1], [NaN]] }, { data: [[1]] }, { data: [[1], [2], [3]] }, { data: [[], []] }, { data: [[1, 2], [3]] }, { shape: [2, 3], data: [[1, 2], [3, 4]] }, { shape: [3, 2], data: [[1, 2], [3, 4]] }, [], "s", null]) {
    assert.equal((await emb(bad)).error?.tag, "BadReply", JSON.stringify(bad));
  }
  assert.deepEqual((await emb(new Error("5004: Invalid data"))).error, { tag: "InvalidInput", message: "5004: Invalid data" });
  assert.deepEqual((await emb(new Error("3036: used up"))).error, { tag: "QuotaExceeded" });
  await assert.rejects(emb({}, []), /at least one text/);
});

// #3478: `runStream`, against a fake whose reply is a `ReadableStream` of the
// bytes of a Server-Sent Events body, cut into the chunks the test chooses.
const sseBody = (chunks, { failAfter, onCancel, hold } = {}) => {
  const enc = new TextEncoder();
  let i = 0;
  return new ReadableStream({
    pull(controller) {
      if (failAfter !== undefined && i === failAfter) return controller.error(new Error("connection reset"));
      if (i < chunks.length) controller.enqueue(typeof chunks[i] === "string" ? enc.encode(chunks[i]) : chunks[i]);
      else if (hold) return new Promise(() => {});
      else controller.close();
      i += 1;
    },
    cancel() { onCancel?.(); },
  });
};
const collect = async (iterable) => {
  const out = [];
  for await (const e of iterable) out.push(e);
  return out;
};
const events = async (chunks, opts) => {
  const r = await new WorkersAi({ AI: fakeAi(sseBody(chunks, opts)) }).runStream("@cf/x/y", "{}");
  assert.equal(r.tag, "Ok");
  return collect(r.value);
};

t("WorkersAi.runStream: asks the model to stream and yields each event's data text, consuming [DONE]", async () => {
  const ai = fakeAi(sseBody(['data: {"response":"he"}\n\n', 'data: {"response":"llo"}\n\n', "data: [DONE]\n\n"]));
  const r = await new WorkersAi({ AI: ai }).runStream("@cf/meta/llama-3.2-1b-instruct", '{"prompt":"hi","stream":false}');
  assert.deepEqual(await collect(r.value), ['{"response":"he"}', '{"response":"llo"}']);
  // `stream: true` wins over whatever the input said.
  assert.deepEqual(ai.calls[0].input, { prompt: "hi", stream: true });
});

t("WorkersAi.runStream: decodes events however the bytes are cut, and accepts LF, CRLF and CR", async () => {
  const whole = 'data: {"a":"é"}\r\n\r\ndata: two\n\ndata: three\r\r';
  const bytes = new TextEncoder().encode(whole);
  // One byte at a time: boundaries fall inside a multi-byte character and inside a CRLF.
  const single = Array.from(bytes, (b) => new Uint8Array([b]));
  assert.deepEqual(await events(single), ['{"a":"é"}', "two", "three"]);
  assert.deepEqual(await events([whole]), ['{"a":"é"}', "two", "three"]);
});

t("WorkersAi.runStream: joins data lines, ignores other fields and comments, and gives a last event with no blank line after it", async () => {
  assert.deepEqual(await events([": keep-alive\n", "event: message\nid: 7\nretry: 5\ndata: a\ndata: b\n\n", "data:c\n\n", "data: last"]), ["a\nb", "c", "last"]);
  assert.deepEqual(await events(["\n\n\n: only a comment\n\n"]), []);
  assert.deepEqual(await events([]), []);
  // `[DONE]` ends the stream: nothing after it is read.
  assert.deepEqual(await events(["data: one\n\ndata: [DONE]\n\ndata: late\n\n"]), ["one"]);
});

t("WorkersAi.runStream: a failure before the stream is an Err, a model that does not stream is AiFailed, and the binding faults on a bad input", async () => {
  const run = (reply, input = "{}") => new WorkersAi({ AI: fakeAi(reply) }).runStream("@cf/x/y", input);
  assert.deepEqual((await run(new Error("5007: No such model"))).error, { tag: "ModelNotFound" });
  assert.deepEqual((await run(new Error("3036: used up"))).error, { tag: "QuotaExceeded" });
  assert.equal((await run({ response: "not a stream" })).error.tag, "AiFailed");
  await assert.rejects(run({}, "[1]"), /`input`/);
  await assert.rejects(run({}, "{}").then(() => new WorkersAi({}).runStream("@cf/x/y", "{}")), /\[ai\]/);
});

t("WorkersAi.runStream: a failure after the first event faults the stream, after the events already given", async () => {
  const r = await new WorkersAi({ AI: fakeAi(sseBody(["data: one\n\n", "data: two\n\n", "data: three\n\n"], { failAfter: 2 })) }).runStream("@cf/x/y", "{}");
  const seen = [];
  await assert.rejects(async () => { for await (const e of r.value) seen.push(e); }, /connection reset/);
  assert.deepEqual(seen, ["one", "two"]);
});

t("WorkersAi.runStream: return() cancels at once even while a read is in flight, and the pending next() ends", async () => {
  // A model that is only sending keep-alive comments: `next()` is parked in a
  // read that yields no event. A generator would queue `return()` behind it.
  let cancels = 0;
  const body = sseBody([": keep-alive\n"], { hold: true, onCancel: () => { cancels += 1; } });
  const r = await new WorkersAi({ AI: fakeAi(body) }).runStream("@cf/x/y", "{}");
  const iterator = r.value[Symbol.asyncIterator]();
  const pending = iterator.next();
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.deepEqual(await iterator.return(), { done: true, value: undefined });
  assert.equal(cancels, 1);
  assert.deepEqual(await pending, { done: true, value: undefined });
  // Done stays done.
  assert.deepEqual(await iterator.next(), { done: true, value: undefined });
});

t("WorkersAi.runStream: abandoning the stream, or reaching [DONE], cancels the model's", async () => {
  let cancels = 0;
  const onCancel = () => { cancels += 1; };
  const abandoned = await new WorkersAi({ AI: fakeAi(sseBody(["data: one\n\n", "data: two\n\n"], { onCancel, hold: true })) }).runStream("@cf/x/y", "{}");
  for await (const e of abandoned.value) { assert.equal(e, "one"); break; }
  assert.equal(cancels, 1);
  await events(["data: one\n\ndata: [DONE]\n\n"], { onCancel, hold: true });
  assert.equal(cancels, 2);
});

// #3478: the one-time warning of a remote-only binding under `bynk dev`.
t("WorkersAi.run: warns once under bynk dev unless the project acknowledged it, and never in a deployment", async () => {
  const warnings = [];
  const original = console.warn;
  console.warn = (...args) => warnings.push(args.join(" "));
  try {
    const run = async (env) => new WorkersAi({ AI: fakeAi({ response: "x" }), ...env }).run("@cf/x/y", "{}");
    // Deployed: neither variable, so silence.
    assert.equal((await run({})).tag, "Ok");
    assert.deepEqual(warnings, []);
    // Acknowledged: `[dev] ai = "allow"` sets the variable.
    assert.equal((await run({ BYNK_DEV: "1", BYNK_DEV_ACK_AI: "allow" })).tag, "Ok");
    assert.deepEqual(warnings, []);
    // `bynk dev` without it: the call goes ahead, with a warning that links to Cloudflare and states no price.
    assert.equal((await run({ BYNK_DEV: "1" })).tag, "Ok");
    assert.equal(warnings.length, 1);
    assert.match(warnings[0], /may incur charges/);
    assert.match(warnings[0], /https:\/\/developers\.cloudflare\.com\/workers-ai\/platform\/pricing\//);
    assert.match(warnings[0], /ai = "allow"/);
    assert.doesNotMatch(warnings[0], /\$|free|neuron/i);
    // Once: a second call says nothing more.
    assert.equal((await run({ BYNK_DEV: "1" })).tag, "Ok");
    assert.equal(warnings.length, 1);
  } finally {
    console.warn = original;
  }
});

// #3301: `WorkersStoredSecret`, against a fake of Cloudflare's Secrets Store
// secret binding: `{ get() }`, which resolves the secret's value.
t("WorkersStoredSecret.get: reads the instance's own binding and returns the value unchanged", async () => {
  const calls = [];
  const secret = (value) => ({
    async get() {
      calls.push(value);
      return value;
    },
  });
  const env = { SECRET_STRIPE_KEY: secret("sk_one"), SECRET_WEBHOOK_KEY: secret("whsec_two") };
  assert.equal(await new WorkersStoredSecret(env, "SECRET_STRIPE_KEY").get(), "sk_one");
  assert.equal(await new WorkersStoredSecret(env, "SECRET_WEBHOOK_KEY").get(), "whsec_two");
  // Each read goes to the platform; the provider keeps no copy.
  assert.equal(await new WorkersStoredSecret(env, "SECRET_STRIPE_KEY").get(), "sk_one");
  assert.deepEqual(calls, ["sk_one", "whsec_two", "sk_one"]);
});

t("WorkersStoredSecret.get: a missing binding names it and the stanza, and a platform refusal propagates", async () => {
  await assert.rejects(
    new WorkersStoredSecret({}, "SECRET_STRIPE_KEY").get(),
    /requires a Secrets Store binding \(env\.SECRET_STRIPE_KEY\).*\[\[secrets_store_secrets\]\]/,
  );
  const refusing = {
    SECRET_STRIPE_KEY: {
      async get() {
        throw new Error('Secret "stripe-live" not found');
      },
    },
  };
  await assert.rejects(new WorkersStoredSecret(refusing, "SECRET_STRIPE_KEY").get(), /not found/);
});

// #3302: `WorkersEmail`, against a fake of Cloudflare's `send_email` binding:
// `send(builder)` resolves `{ messageId }`, or rejects with an `Error` whose
// `code` names the refusal.
const mail = (over = {}) => ({
  from: "orders@shop.example",
  to: ["a@buyer.example"],
  cc: [],
  bcc: [],
  replyTo: None,
  subject: "Your order",
  text: Some("Thanks"),
  html: None,
  attachments: [],
  headers: new Map(),
  ...over,
});
const emailEnv = (impl) => {
  const sent = [];
  return {
    sent,
    env: {
      EMAIL_MAIL: {
        async send(builder) {
          sent.push(builder);
          return impl ? impl(builder) : { messageId: "msg-1" };
        },
      },
    },
  };
};
const refusing = (code, message = "refused") => () => {
  const e = new Error(message);
  e.code = code;
  throw e;
};

t("WorkersEmail.send: builds the platform's builder and returns the message id", async () => {
  const { env, sent } = emailEnv();
  const r = await new WorkersEmail(env, "EMAIL_MAIL").send(
    mail({
      cc: ["c@buyer.example"],
      bcc: ["d@buyer.example"],
      replyTo: Some("help@shop.example"),
      html: Some("<p>Thanks</p>"),
      headers: new Map([["X-Order", "42"]]),
    }),
  );
  assert.deepEqual(r, { tag: "Ok", value: "msg-1" });
  assert.deepEqual(sent, [
    {
      from: "orders@shop.example",
      to: ["a@buyer.example"],
      subject: "Your order",
      cc: ["c@buyer.example"],
      bcc: ["d@buyer.example"],
      replyTo: "help@shop.example",
      text: "Thanks",
      html: "<p>Thanks</p>",
      headers: { "X-Order": "42" },
    },
  ]);
});

t("WorkersEmail.send: an attachment goes as base64 text, with its disposition and content id", async () => {
  const { env, sent } = emailEnv();
  const bytes = new Uint8Array([0, 1, 2, 250, 255]);
  await new WorkersEmail(env, "EMAIL_MAIL").send(
    mail({
      attachments: [
        { filename: "a.pdf", contentType: "application/pdf", content: bytes, inline: false },
        { filename: "logo.png", contentType: "image/png", content: bytes, inline: true },
      ],
    }),
  );
  const b64 = Buffer.from(bytes).toString("base64");
  assert.deepEqual(sent[0].attachments, [
    { content: b64, filename: "a.pdf", type: "application/pdf", disposition: "attachment" },
    { content: b64, filename: "logo.png", type: "image/png", disposition: "inline", contentId: "logo.png" },
  ]);
  // Never an ArrayBuffer or a view: the local simulator cannot serialise one.
  assert.equal(typeof sent[0].attachments[0].content, "string");
});

t("WorkersEmail.send: each platform error code maps to its variant, any other to SendFailed", async () => {
  const cases = [
    ["E_SENDER_NOT_VERIFIED", { tag: "SenderNotVerified" }],
    ["E_RECIPIENT_NOT_ALLOWED", { tag: "RecipientNotAllowed" }],
    ["E_RATE_LIMIT_EXCEEDED", { tag: "RateLimited" }],
    ["E_DAILY_LIMIT_EXCEEDED", { tag: "DailyLimit" }],
    ["E_SOMETHING_NEW", { tag: "SendFailed", message: "boom" }],
  ];
  for (const [code, variant] of cases) {
    const { env } = emailEnv(refusing(code, "boom"));
    assert.deepEqual(await new WorkersEmail(env, "EMAIL_MAIL").send(mail()), { tag: "Err", error: variant }, code);
  }
  // An error with no code is a `SendFailed` too.
  const { env } = emailEnv(() => {
    throw new Error("network");
  });
  assert.deepEqual(await new WorkersEmail(env, "EMAIL_MAIL").send(mail()), {
    tag: "Err",
    error: { tag: "SendFailed", message: "network" },
  });
});

t("WorkersEmail.send: a missing binding names it and the stanza", async () => {
  await assert.rejects(
    new WorkersEmail({}, "EMAIL_MAIL").send(mail()),
    /requires an Email Service binding \(env\.EMAIL_MAIL\).*\[\[send_email\]\]/,
  );
});

// A limit breach faults before the call: nothing is sent.
const faults = async (message, pattern) => {
  const { env, sent } = emailEnv();
  await assert.rejects(new WorkersEmail(env, "EMAIL_MAIL").send(message), pattern);
  assert.deepEqual(sent, []);
};
const addresses = (n, tag = "x") => Array.from({ length: n }, (_, i) => `${tag}${i}@buyer.example`);

t("WorkersEmail.send: 50 recipients across to, cc and bcc pass and 51 fault", async () => {
  const ok = emailEnv();
  const r = await new WorkersEmail(ok.env, "EMAIL_MAIL").send(
    mail({ to: addresses(20, "t"), cc: addresses(20, "c"), bcc: addresses(10, "b") }),
  );
  assert.equal(r.tag, "Ok");
  await faults(
    mail({ to: addresses(20, "t"), cc: addresses(20, "c"), bcc: addresses(11, "b") }),
    /at most 50 recipients across to, cc and bcc, not 51/,
  );
});

t("WorkersEmail.send: 32 attachments pass and 33 fault", async () => {
  const file = (i) => ({ filename: `f${i}`, contentType: "text/plain", content: new Uint8Array(1), inline: false });
  const ok = emailEnv();
  const r = await new WorkersEmail(ok.env, "EMAIL_MAIL").send(mail({ attachments: Array.from({ length: 32 }, (_, i) => file(i)) }));
  assert.equal(r.tag, "Ok");
  await faults(mail({ attachments: Array.from({ length: 33 }, (_, i) => file(i)) }), /at most 32 attachments, not 33/);
});

t("WorkersEmail.send: 25 MiB in total, measured as sent (attachments as base64), passes and one byte more faults", async () => {
  const limit = 25 * 1024 * 1024;
  const file = (n) => ({ filename: "big", contentType: "application/octet-stream", content: new Uint8Array(n), inline: false });
  const body = (n, over = {}) => mail({ subject: "", text: Some(""), attachments: [file(n)], ...over });
  // 3n raw bytes are 4n as base64: 19 MiB raw is 25.33 MiB sent, so it faults.
  const rawAtLimit = (limit / 4) * 3;
  const ok = emailEnv();
  assert.equal((await new WorkersEmail(ok.env, "EMAIL_MAIL").send(body(rawAtLimit))).tag, "Ok");
  await faults(body(rawAtLimit + 3), /totals at most 25 MiB as sent/);
  await faults(body(19 * 1024 * 1024 + 512 * 1024), /totals at most 25 MiB as sent/);
  // Text, html and custom headers count too.
  await faults(mail({ text: Some("x".repeat(limit)), html: Some("y") }), /totals at most 25 MiB/);
  await faults(body(rawAtLimit, { headers: new Map([["X-A", "v"]]) }), /totals at most 25 MiB/);
});

t("WorkersEmail.send: a message with no recipient faults", async () => {
  await faults(mail({ to: [], cc: [], bcc: [] }), /at least one recipient/);
  const ok = emailEnv();
  assert.equal((await new WorkersEmail(ok.env, "EMAIL_MAIL").send(mail({ to: [], cc: [], bcc: ["a@x.example"] }))).tag, "Ok");
});

t("WorkersEmail.send: neither text nor html faults, and either alone passes", async () => {
  await faults(mail({ text: None, html: None }), /needs text, html or both/);
  const ok = emailEnv();
  assert.equal((await new WorkersEmail(ok.env, "EMAIL_MAIL").send(mail({ text: None, html: Some("<p>x</p>") }))).tag, "Ok");
});

// R-16: header injection. Each reserved name, in any case, and a line break in
// a name or a value, faults before the call; the fault never echoes a value.
t("WorkersEmail.send: each reserved header faults, whatever its case", async () => {
  for (const name of ["From", "To", "Cc", "Bcc", "Subject", "Reply-To", "from", "TO", "rEpLy-tO", "SUBJECT"]) {
    await faults(mail({ headers: new Map([[name, "x"]]) }), /set by the message's own fields and may not be set in headers/);
  }
  // A look-alike that is not reserved passes.
  const ok = emailEnv();
  assert.equal((await new WorkersEmail(ok.env, "EMAIL_MAIL").send(mail({ headers: new Map([["X-To", "x"], ["Reply-To-Id", "y"]]) }))).tag, "Ok");
});

t("WorkersEmail.send: a line break in a header name or value faults, and the value is not echoed", async () => {
  for (const bad of ["a\r\nBcc: evil@x.example", "a\nBcc: evil@x.example", "a\rBcc: evil@x.example"]) {
    await faults(mail({ headers: new Map([[bad, "x"]]) }), /a header name may not contain a line break/);
    const { env, sent } = emailEnv();
    let message = "";
    try {
      await new WorkersEmail(env, "EMAIL_MAIL").send(mail({ headers: new Map([["X-Note", `v${bad}`]]) }));
    } catch (e) {
      message = e.message;
    }
    assert.match(message, /the value of header "X-Note" may not contain a line break/);
    assert.ok(!message.includes("evil@x.example"), message);
    assert.deepEqual(sent, []);
  }
});

t("WorkersEmail.send: a control character or Unicode line separator in a header value faults", async () => {
  for (const bad of ["\u0000", "\u0001", "\u000b", "\u007f", "\u0085", "\u2028", "\u2029"]) {
    await faults(mail({ headers: new Map([["X-Note", `a${bad}b`]]) }), /may not contain a (line break|control character)/);
  }
  // A tab is ordinary header whitespace.
  const ok = emailEnv();
  assert.equal((await new WorkersEmail(ok.env, "EMAIL_MAIL").send(mail({ headers: new Map([["X-Note", "a\tb"]]) }))).tag, "Ok");
});

t("WorkersEmail.send: an empty or malformed header name faults, and 16 KB of headers is the ceiling", async () => {
  for (const bad of ["", "X Bad", "X:Bad", "X\u0000Bad"]) {
    await faults(mail({ headers: new Map([[bad, "x"]]) }), /is not a header name/);
  }
  const ok = emailEnv();
  assert.equal((await new WorkersEmail(ok.env, "EMAIL_MAIL").send(mail({ headers: new Map([["X-A", "v".repeat(16 * 1024 - 3)]]) }))).tag, "Ok");
  await faults(mail({ headers: new Map([["X-A", "v".repeat(16 * 1024 - 2)]]) }), /custom headers total at most 16 KB/);
});

t("WorkersEmail.send: a fault never carries the message body or an attachment's content", async () => {
  const secretBody = "TOP-SECRET-BODY";
  const secretFile = new Uint8Array(Buffer.from("TOP-SECRET-FILE"));
  const { env } = emailEnv();
  let seen = "";
  try {
    await new WorkersEmail(env, "EMAIL_MAIL").send(
      mail({
        text: Some(secretBody),
        to: addresses(51),
        attachments: [{ filename: "f", contentType: "text/plain", content: secretFile, inline: false }],
      }),
    );
  } catch (e) {
    seen = e.message;
  }
  assert.ok(seen.length > 0);
  assert.ok(!seen.includes(secretBody) && !seen.includes("TOP-SECRET-FILE") && !seen.includes(Buffer.from(secretFile).toString("base64")), seen);
});

// Hyperdrive H1 (#3318): `WorkersPostgres`, against a fake of the executor a
// Worker's compose hands it (the runtime's `pgExecute`) and a fake of the
// Hyperdrive binding. The binding itself never touches the driver.
const SECRET_URL = "postgres://app:s3cr3t@db.internal.example:5432/shop";

function pgEnv(answer) {
  const calls = [];
  const execute = async (connectionString, sql, params) => {
    calls.push({ connectionString, sql, params });
    return answer(sql, params);
  };
  // A transaction on the fake: each statement is answered in turn on a copy of
  // the world, which is kept only if none fails (the executor's `sql.begin`).
  execute.transaction = async (connectionString, statements) => {
    const counts = [];
    for (const s of statements) {
      calls.push({ connectionString, sql: s.sql, params: s.params, inTransaction: true });
      const out = answer(s.sql, s.params);
      if (!out.ok) return out;
      counts.push(out.rowCount);
    }
    return { ok: true, rowCounts: counts };
  };
  return { env: { HYPERDRIVE_MAIN: { connectionString: SECRET_URL } }, execute, calls };
}

const pgOk = (rows, rowCount = rows.length) => ({ ok: true, rows, rowCount });
const pgFail = (kind, sqlstate, message) => ({ ok: false, failure: { kind, sqlstate, message } });

t("WorkersPostgres.all: gives every row as JSON text, reading the connection string from the binding", async () => {
  const { env, execute, calls } = pgEnv(() => pgOk([{ id: "9007199254740993", name: "a" }, { id: "2", name: "b" }]));
  const r = await new WorkersPostgres(env, "HYPERDRIVE_MAIN", execute).all("select id, name from t where id > $1", [{ tag: "PgInteger", value: 0 }]);
  assert.equal(r.tag, "Ok");
  assert.deepEqual(JSON.parse(r.value), [{ id: "9007199254740993", name: "a" }, { id: "2", name: "b" }]);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].connectionString, SECRET_URL);
  assert.equal(calls[0].sql, "select id, name from t where id > $1");
  assert.deepEqual(calls[0].params, [{ tag: "PgInteger", value: 0 }]);
});

t("WorkersPostgres.first: the first row, or None when there is none", async () => {
  const some = pgEnv(() => pgOk([{ n: 1 }, { n: 2 }]));
  const a = await new WorkersPostgres(some.env, "HYPERDRIVE_MAIN", some.execute).first("select n from t", []);
  assert.equal(a.tag, "Ok");
  assert.equal(a.value.tag, "Some");
  assert.deepEqual(JSON.parse(a.value.value), { n: 1 });
  const none = pgEnv(() => pgOk([]));
  const b = await new WorkersPostgres(none.env, "HYPERDRIVE_MAIN", none.execute).first("select n from t", []);
  assert.deepEqual([b.tag, b.value.tag], ["Ok", "None"]);
});

t("WorkersPostgres.run: reports the row count", async () => {
  const { env, execute } = pgEnv(() => pgOk([], 3));
  const r = await new WorkersPostgres(env, "HYPERDRIVE_MAIN", execute).run("delete from t", []);
  assert.deepEqual([r.tag, r.value], ["Ok", { rowCount: 3 }]);
});

t("WorkersPostgres: each failure kind is its PgError variant, carrying the SQLSTATE where there is one", async () => {
  const cases = [
    [pgFail("constraint", "23505", "duplicate key"), { tag: "PgConstraint", sqlstate: "23505", message: "duplicate key" }],
    [pgFail("serialization", "40001", "could not serialize"), { tag: "PgSerialization", message: "could not serialize" }],
    [pgFail("connection", "", "could not reach the database (CONNECTION_CLOSED)"), { tag: "PgConnection", message: "could not reach the database (CONNECTION_CLOSED)" }],
    [pgFail("failed", "42P01", "no such table"), { tag: "PgFailed", sqlstate: "42P01", message: "no such table" }],
  ];
  for (const [answer, error] of cases) {
    const { env, execute } = pgEnv(() => answer);
    const pg = new WorkersPostgres(env, "HYPERDRIVE_MAIN", execute);
    for (const r of [await pg.all("x", []), await pg.first("x", []), await pg.run("x", [])]) {
      assert.equal(r.tag, "Err");
      assert.deepEqual(r.error, error);
    }
  }
});

t("WorkersPostgres: a missing binding or driver faults, naming the binding and never a value", async () => {
  const noBinding = new WorkersPostgres({}, "HYPERDRIVE_MAIN", async () => pgOk([]));
  await assert.rejects(() => noBinding.all("x", []), /requires a Hyperdrive binding \(env\.HYPERDRIVE_MAIN\)/);
  const emptyString = new WorkersPostgres({ HYPERDRIVE_MAIN: { connectionString: "" } }, "HYPERDRIVE_MAIN", async () => pgOk([]));
  await assert.rejects(() => emptyString.run("x", []), /requires a Hyperdrive binding/);
  const noDriver = new WorkersPostgres({ HYPERDRIVE_MAIN: { connectionString: SECRET_URL } }, "HYPERDRIVE_MAIN");
  let seen = "";
  try {
    await noDriver.first("x", []);
  } catch (e) {
    seen = e.message;
  }
  assert.match(seen, /needs the Postgres driver/);
  assert.ok(!seen.includes("s3cr3t") && !seen.includes(SECRET_URL), seen);
});

t("WorkersPostgres: an integration binding's faulting connection string surfaces as a fault", async () => {
  const env = { HYPERDRIVE_MAIN: { get connectionString() { throw new Error("bynk: Hyperdrive binding `HYPERDRIVE_MAIN` has no database in a test"); } } };
  await assert.rejects(() => new WorkersPostgres(env, "HYPERDRIVE_MAIN", async () => pgOk([])).all("x", []), /no database in a test/);
});

t("WorkersPostgres: the connection string reaches the executor and nothing else", async () => {
  const { env, execute } = pgEnv(() => pgFail("failed", "28P01", "password authentication failed for user app"));
  const r = await new WorkersPostgres(env, "HYPERDRIVE_MAIN", execute).run("x", []);
  assert.ok(!JSON.stringify(r).includes("s3cr3t") && !JSON.stringify(r).includes(SECRET_URL));
});

// Hyperdrive H3 (#3320): `WorkersPostgres.transaction`.
t("WorkersPostgres.transaction: runs the statements in order in one transaction and reports each row count", async () => {
  const { env, execute, calls } = pgEnv((sql) => pgOk([], sql.startsWith("insert") ? 1 : 5));
  const r = await new WorkersPostgres(env, "HYPERDRIVE_MAIN", execute).transaction([
    { sql: "insert into a values ($1)", params: [{ tag: "PgInteger", value: 1 }] },
    { sql: "update b set n = n + 1", params: [] },
  ]);
  assert.deepEqual([r.tag, r.value], ["Ok", [{ rowCount: 1 }, { rowCount: 5 }]]);
  assert.deepEqual(calls.map((c) => [c.sql, c.inTransaction, c.connectionString]), [
    ["insert into a values ($1)", true, SECRET_URL],
    ["update b set n = n + 1", true, SECRET_URL],
  ]);
  assert.deepEqual(calls[0].params, [{ tag: "PgInteger", value: 1 }]);
});

t("WorkersPostgres.transaction: a failure in the second statement rolls back the first and is the result", async () => {
  // A table of rows, and an executor whose transaction applies to a copy and
  // keeps it only when every statement succeeded (what `sql.begin` guarantees).
  let committed = [];
  const env = { HYPERDRIVE_MAIN: { connectionString: SECRET_URL } };
  const execute = async () => pgOk([]);
  execute.transaction = async (_cs, statements) => {
    const scratch = [...committed];
    const counts = [];
    for (const s of statements) {
      if (s.sql === "insert into stock values ($1)") {
        if (scratch.includes(s.params[0].value)) return pgFail("constraint", "23505", "duplicate key value violates unique constraint");
        scratch.push(s.params[0].value);
        counts.push(1);
      }
    }
    committed = scratch;
    return { ok: true, rowCounts: counts };
  };
  const insert = (n) => ({ sql: "insert into stock values ($1)", params: [{ tag: "PgInteger", value: n }] });
  const pg = new WorkersPostgres(env, "HYPERDRIVE_MAIN", execute);
  assert.equal((await pg.transaction([insert(1)])).tag, "Ok");
  // 2 is new, 1 is a duplicate: the second statement fails, so 2 must not stay.
  const r = await pg.transaction([insert(2), insert(1)]);
  assert.equal(r.tag, "Err");
  assert.deepEqual(r.error, { tag: "PgConstraint", sqlstate: "23505", message: "duplicate key value violates unique constraint" });
  assert.deepEqual(committed, [1]);
});

t("WorkersPostgres.transaction: each failure kind is its PgError variant; the connection string never leaks", async () => {
  for (const [answer, tag] of [
    [pgFail("serialization", "40001", "could not serialize"), "PgSerialization"],
    [pgFail("connection", "", "could not reach the database (CONNECTION_CLOSED)"), "PgConnection"],
    [pgFail("failed", "28P01", "password authentication failed for user app"), "PgFailed"],
  ]) {
    const { env, execute } = pgEnv(() => answer);
    const r = await new WorkersPostgres(env, "HYPERDRIVE_MAIN", execute).transaction([{ sql: "x", params: [] }]);
    assert.equal(r.tag, "Err");
    assert.equal(r.error.tag, tag);
    assert.ok(!JSON.stringify(r).includes("s3cr3t") && !JSON.stringify(r).includes(SECRET_URL));
  }
});

t("WorkersPostgres.transaction: an empty list is Ok([]) and opens nothing; a missing binding or driver still faults", async () => {
  const { env, execute, calls } = pgEnv(() => pgOk([]));
  const r = await new WorkersPostgres(env, "HYPERDRIVE_MAIN", execute).transaction([]);
  assert.deepEqual([r.tag, r.value], ["Ok", []]);
  assert.equal(calls.length, 0);
  await assert.rejects(() => new WorkersPostgres({}, "HYPERDRIVE_MAIN", execute).transaction([]), /requires a Hyperdrive binding/);
  await assert.rejects(
    () => new WorkersPostgres({ HYPERDRIVE_MAIN: { connectionString: SECRET_URL } }, "HYPERDRIVE_MAIN").transaction([{ sql: "x", params: [] }]),
    /needs the Postgres driver/,
  );
});

// Hyperdrive H5 (#3322): `WorkersMySql`, against a fake of the executor a
// Worker's compose hands it (the runtime's `mysqlExecute`) and a fake of the
// Hyperdrive binding, read from its parts (Cloudflare's mysql2 guide).
const MY_PASSWORD = "my-s3cr3t-pw";
const MY_BINDING = { host: "db.internal.example", port: 3306, user: "app", password: MY_PASSWORD, database: "shop" };

function myEnv(answer) {
  const calls = [];
  const execute = async (endpoint, sql, params) => {
    calls.push({ endpoint, sql, params });
    return answer(sql, params);
  };
  execute.transaction = async (endpoint, statements) => {
    const runs = [];
    for (const s of statements) {
      calls.push({ endpoint, sql: s.sql, params: s.params, inTransaction: true });
      const out = answer(s.sql, s.params);
      if (!out.ok) return out;
      runs.push({ rowCount: out.rowCount, insertId: out.insertId });
    }
    return { ok: true, runs };
  };
  return { env: { HYPERDRIVE_LEGACY: MY_BINDING }, execute, calls };
}

const myOk = (rows, rowCount = rows.length, insertId = 0) => ({ ok: true, rows, rowCount, insertId });
const myFail = (kind, code, message) => ({ ok: false, failure: { kind, code, message } });

t("WorkersMySql.all: gives every row as JSON text, reading the endpoint from the binding's parts", async () => {
  const { env, execute, calls } = myEnv(() => myOk([{ id: 1, name: "a" }, { id: 2, name: "b" }]));
  const r = await new WorkersMySql(env, "HYPERDRIVE_LEGACY", execute).all("select id, name from t where id > ?", [{ tag: "MySqlInteger", value: 0 }]);
  assert.equal(r.tag, "Ok");
  assert.deepEqual(JSON.parse(r.value), [{ id: 1, name: "a" }, { id: 2, name: "b" }]);
  assert.deepEqual(calls[0].endpoint, MY_BINDING);
  assert.equal(calls[0].sql, "select id, name from t where id > ?");
  assert.deepEqual(calls[0].params, [{ tag: "MySqlInteger", value: 0 }]);
});

t("WorkersMySql: a binding with only a connection string is read from that", async () => {
  const { execute, calls } = myEnv(() => myOk([]));
  const env = { HYPERDRIVE_LEGACY: { connectionString: "mysql://app:p%40ss@db.example:3307/shop%20db" } };
  await new WorkersMySql(env, "HYPERDRIVE_LEGACY", execute).all("select 1", []);
  assert.deepEqual(calls[0].endpoint, { host: "db.example", port: 3307, user: "app", password: "p@ss", database: "shop db" });
});

t("WorkersMySql.first: the first row, or None when there is none", async () => {
  const some = myEnv(() => myOk([{ n: 1 }, { n: 2 }]));
  const a = await new WorkersMySql(some.env, "HYPERDRIVE_LEGACY", some.execute).first("select n from t", []);
  assert.deepEqual(JSON.parse(a.value.value), { n: 1 });
  const none = myEnv(() => myOk([]));
  const b = await new WorkersMySql(none.env, "HYPERDRIVE_LEGACY", none.execute).first("select n from t", []);
  assert.equal(b.tag, "Ok");
  assert.equal(b.value, None);
});

t("WorkersMySql.run: reports the row count and the insert id", async () => {
  const { env, execute } = myEnv(() => myOk([], 3, 41));
  const r = await new WorkersMySql(env, "HYPERDRIVE_LEGACY", execute).run("insert into t values (?)", [{ tag: "MySqlText", value: "x" }]);
  assert.deepEqual(r, { tag: "Ok", value: { rowCount: 3, insertId: 41 } });
});

t("WorkersMySql: each failure kind is its MySqlError variant, carrying the error number where there is one", async () => {
  const cases = [
    [myFail("constraint", 1062, "Duplicate entry"), { tag: "MySqlConstraint", code: 1062, message: "Duplicate entry" }],
    [myFail("serialization", 1213, "Deadlock found"), { tag: "MySqlSerialization", code: 1213, message: "Deadlock found" }],
    [myFail("connection", 0, "could not reach the database"), { tag: "MySqlConnection", message: "could not reach the database" }],
    [myFail("failed", 1064, "syntax error"), { tag: "MySqlFailed", code: 1064, message: "syntax error" }],
  ];
  for (const [out, expected] of cases) {
    const { env, execute } = myEnv(() => out);
    const r = await new WorkersMySql(env, "HYPERDRIVE_LEGACY", execute).run("x", []);
    assert.deepEqual(r, { tag: "Err", error: expected });
  }
});

t("WorkersMySql: a missing binding or driver faults, naming the binding and never a value", async () => {
  const noBinding = new WorkersMySql({}, "HYPERDRIVE_LEGACY", async () => myOk([]));
  await assert.rejects(() => noBinding.all("x", []), /requires a Hyperdrive binding \(env\.HYPERDRIVE_LEGACY\)/);
  const noDriver = new WorkersMySql({ HYPERDRIVE_LEGACY: MY_BINDING }, "HYPERDRIVE_LEGACY");
  let seen = "";
  try {
    await noDriver.all("x", []);
  } catch (e) {
    seen = e.message;
  }
  assert.match(seen, /needs the MySQL driver/);
  assert.ok(!seen.includes(MY_PASSWORD), seen);
});

t("WorkersMySql: an integration binding's faulting connection string surfaces as a fault", async () => {
  const env = { HYPERDRIVE_LEGACY: { get connectionString() { throw new Error("bynk: Hyperdrive binding `HYPERDRIVE_LEGACY` has no database in a test"); } } };
  await assert.rejects(() => new WorkersMySql(env, "HYPERDRIVE_LEGACY", async () => myOk([])).all("x", []), /no database in a test/);
});

t("WorkersMySql: the password reaches the executor and nothing else", async () => {
  const { env, execute } = myEnv(() => myFail("failed", 1045, "Access denied for user app"));
  const r = await new WorkersMySql(env, "HYPERDRIVE_LEGACY", execute).run("x", []);
  assert.ok(!JSON.stringify(r).includes(MY_PASSWORD));
});

t("WorkersMySql.transaction: runs the statements in order and reports each run", async () => {
  const { env, execute, calls } = myEnv((sql) => (sql === "A" ? myOk([], 1, 7) : myOk([], 2, 0)));
  const r = await new WorkersMySql(env, "HYPERDRIVE_LEGACY", execute).transaction([
    { sql: "A", params: [] },
    { sql: "B", params: [{ tag: "MySqlNull" }] },
  ]);
  assert.deepEqual(r, { tag: "Ok", value: [{ rowCount: 1, insertId: 7 }, { rowCount: 2, insertId: 0 }] });
  assert.deepEqual(calls.map((c) => c.sql), ["A", "B"]);
  assert.ok(calls.every((c) => c.inTransaction));
});

t("WorkersMySql.transaction: a failure is the result; an empty list is Ok([]) and opens nothing; a missing binding or driver still faults", async () => {
  const bad = myEnv((sql) => (sql === "B" ? myFail("constraint", 1062, "Duplicate entry") : myOk([], 1)));
  const r = await new WorkersMySql(bad.env, "HYPERDRIVE_LEGACY", bad.execute).transaction([{ sql: "A", params: [] }, { sql: "B", params: [] }]);
  assert.deepEqual(r, { tag: "Err", error: { tag: "MySqlConstraint", code: 1062, message: "Duplicate entry" } });
  const empty = myEnv(() => myOk([]));
  const e = await new WorkersMySql(empty.env, "HYPERDRIVE_LEGACY", empty.execute).transaction([]);
  assert.deepEqual(e, { tag: "Ok", value: [] });
  assert.equal(empty.calls.length, 0);
  await assert.rejects(() => new WorkersMySql({}, "HYPERDRIVE_LEGACY", empty.execute).transaction([]), /requires a Hyperdrive binding/);
  await assert.rejects(() => new WorkersMySql({ HYPERDRIVE_LEGACY: MY_BINDING }, "HYPERDRIVE_LEGACY").transaction([{ sql: "x", params: [] }]), /needs the MySQL driver/);
});

// #3479: `WorkersVectorize`, against a fake of Cloudflare's Vectorize binding.
// The error messages the platform throws are not documented, so the mapping is
// pinned here against messages of the shapes the binding recognises.
function fakeIndex(over = {}) {
  const calls = [];
  const rec = (name, reply) => async (...args) => {
    calls.push({ name, args });
    if (over[name] instanceof Error) throw over[name];
    return over[name] ?? reply;
  };
  return {
    calls,
    upsert: rec("upsert", { mutationId: "m-1" }),
    insert: rec("insert", { mutationId: "m-2" }),
    query: rec("query", { count: 0, matches: [] }),
    getByIds: rec("getByIds", []),
    deleteByIds: rec("deleteByIds", { mutationId: "m-3" }),
    describe: rec("describe", { dimensions: 3, metric: "cosine" }),
  };
}
const vec = (id, extra = {}) => ({ id, values: [0.1, 0.2, 0.3], metadata: None, namespace: None, ...extra });
const opts = (over = {}) => ({ topK: 5, returnValues: false, returnMetadata: { tag: "Omit" }, namespace: None, filter: [], ...over });
const eq = (field, v) => ({ field, op: { tag: "FilterEq", value: v } });
const metaText = (value) => ({ tag: "MetaText", value });
const metaNum = (value) => ({ tag: "MetaNumber", value });
const range = (field, tag, value) => ({ field, op: { tag, value: metaNum(value) } });
const vz = (index) => new WorkersVectorize({ VECTORIZE_DOCS: index }, "VECTORIZE_DOCS");

t("WorkersVectorize.upsert: sends the platform's vectors and returns the mutation id", async () => {
  const index = fakeIndex();
  const r = await vz(index).upsert([vec("a", { metadata: Some('{"category":"faq"}'), namespace: Some("ns") }), vec("b")]);
  assert.deepEqual(r, { tag: "Ok", value: { id: "m-1" } });
  assert.deepEqual(index.calls[0].args[0], [
    { id: "a", values: [0.1, 0.2, 0.3], namespace: "ns", metadata: { category: "faq" } },
    { id: "b", values: [0.1, 0.2, 0.3] },
  ]);
});

t("WorkersVectorize.insert: sends new vectors and returns the mutation id", async () => {
  const index = fakeIndex();
  assert.deepEqual(await vz(index).insert([vec("c")]), { tag: "Ok", value: { id: "m-2" } });
  assert.equal(index.calls[0].name, "insert");
});

t("WorkersVectorize.upsert: metadata that is not a JSON object faults; a missing binding faults", async () => {
  await assert.rejects(() => vz(fakeIndex()).upsert([vec("a", { metadata: Some("nope") })]), /not JSON text/);
  await assert.rejects(() => vz(fakeIndex()).upsert([vec("a", { metadata: Some("[1]") })]), /must be a JSON object/);
  await assert.rejects(() => new WorkersVectorize({}, "VECTORIZE_DOCS").describe(), /\[\[vectorize\]\]/);
});

t("WorkersVectorize: a platform refusal is a typed error, an unknown index a fault", async () => {
  const err = async (message) => (await vz(fakeIndex({ upsert: new Error(message) })).upsert([vec("a")])).error;
  assert.deepEqual(await err("expected 768 dimensions, got 3"), { tag: "DimensionMismatch", message: "expected 768 dimensions, got 3" });
  assert.deepEqual(await err("429 Too Many Requests"), { tag: "VectorRateLimited" });
  assert.deepEqual(await err("batch is too large"), { tag: "VectorInvalidInput", message: "batch is too large" });
  assert.deepEqual(await err("something else"), { tag: "VectorFailed", message: "something else" });
  await assert.rejects(() => err("index not found"), /index not found/);
});

t("WorkersVectorize.query: builds the platform's options and filter, and shapes the matches", async () => {
  const index = fakeIndex({
    query: { matches: [{ id: "a", score: 0.9, values: [1, 2], metadata: { c: "faq" }, namespace: "ns" }, { id: "b", score: 0.5 }] },
  });
  const filter = [
    eq("category", metaText("faq")),
    range("year", "FilterGte", 2020),
    range("year", "FilterLt", 2030),
    { field: "tag", op: { tag: "FilterAnyOf", values: [metaText("x"), metaText("y")] } },
  ];
  const r = await vz(index).query([1, 2, 3], opts({ topK: 3, returnMetadata: { tag: "Indexed" }, namespace: Some("ns"), filter }));
  assert.deepEqual(index.calls[0].args, [
    [1, 2, 3],
    {
      topK: 3,
      returnValues: false,
      returnMetadata: "indexed",
      namespace: "ns",
      filter: { category: { $eq: "faq" }, year: { $gte: 2020, $lt: 2030 }, tag: { $in: ["x", "y"] } },
    },
  ]);
  assert.deepEqual(r.value, [
    { id: "a", score: 0.9, values: Some([1, 2]), metadata: Some('{"c":"faq"}'), namespace: Some("ns") },
    { id: "b", score: 0.5, values: None, metadata: None, namespace: None },
  ]);
  // No conditions, no filter key.
  await vz(index).query([1], opts());
  assert.equal("filter" in index.calls[1].args[1], false);
});

t("WorkersVectorize.query: a filter the platform cannot take is VectorInvalidInput and no call is made", async () => {
  const index = fakeIndex();
  const refused = async (filter) => (await vz(index).query([1], opts({ filter }))).error;
  for (const filter of [
    [eq("", metaText("x"))],
    [eq('a"b', metaText("x"))],
    [eq("$and", metaText("x"))],
    [eq("k".repeat(513), metaText("x"))],
    [eq("f", { tag: "MetaFlag", value: true }), eq("f", metaText("x"))],
    [range("n", "FilterGt", 1), range("n", "FilterGte", 2)],
    [{ field: "f", op: { tag: "FilterGt", value: { tag: "MetaFlag", value: true } } }],
  ]) {
    assert.equal((await refused(filter)).tag, "VectorInvalidInput");
  }
  assert.equal(index.calls.length, 0);
});

t("WorkersVectorize.getByIds: reads vectors back with values and metadata", async () => {
  const index = fakeIndex({ getByIds: [{ id: "a", values: new Float32Array([0.5]), metadata: { k: 1 } }] });
  const got = await vz(index).getByIds(["a"]);
  assert.deepEqual(got.value, [{ id: "a", values: [0.5], metadata: Some('{"k":1}'), namespace: None }]);
});

t("WorkersVectorize.deleteByIds: returns the mutation id", async () => {
  assert.deepEqual(await vz(fakeIndex()).deleteByIds(["a"]), { tag: "Ok", value: { id: "m-3" } });
});

t("WorkersVectorize.describe: reads the configuration from the top level or under config", async () => {
  assert.deepEqual(await vz(fakeIndex()).describe(), { tag: "Ok", value: { dimensions: 3, metric: "cosine" } });
  const nested = fakeIndex({ describe: { config: { dimensions: 8, metric: "euclidean" } } });
  assert.deepEqual((await vz(nested).describe()).value, { dimensions: 8, metric: "euclidean" });
  const bare = fakeIndex({ describe: { vectorCount: 1 } });
  assert.equal((await vz(bare).describe()).error.tag, "VectorFailed");
});

// #3480: `WorkersAiSearch`, against a fake of Cloudflare's AI Search instance
// binding. The thrown messages are not documented, so the mapping is pinned
// against messages of the shapes the binding recognises; the request and reply
// shapes are the Workers binding page's.
const chunkOf = (id, extra = {}) => ({
  id,
  type: "text",
  score: 0.8,
  text: `text ${id}`,
  item: { key: `docs/${id}.md`, timestamp: 1735689600000, metadata: { lang: "en" } },
  scoring_details: { vector_score: 0.8 },
  ...extra,
});
function fakeSearch(over = {}) {
  const calls = [];
  const rec = (name, reply) => async (...args) => {
    calls.push({ name, args });
    if (over[name] instanceof Error) throw over[name];
    return over[name] ?? reply;
  };
  return {
    calls,
    search: rec("search", { query_kind: "text", search_query: "q", chunks: [chunkOf("a"), chunkOf("b")] }),
    chatCompletions: rec("chatCompletions", {
      id: "c1",
      object: "chat.completion",
      model: "m",
      choices: [{ message: { role: "assistant", content: "Within 30 days." }, finish_reason: "stop" }],
      usage: { prompt_tokens: 12, completion_tokens: 5, total_tokens: 17 },
      chunks: [chunkOf("a")],
    }),
  };
}
const sOpts = (over = {}) => ({
  maxResults: None,
  matchThreshold: None,
  retrieval: None,
  rewriteQuery: false,
  rerank: false,
  filter: [],
  ...over,
});
const as = (instance) => new WorkersAiSearch({ AI_SEARCH_KB: instance }, "AI_SEARCH_KB");
const userMsg = (content) => ({ role: { tag: "User" }, content });

t("WorkersAiSearch.search: sends the query with no options, leaving the platform defaults", async () => {
  const instance = fakeSearch();
  const got = await as(instance).search("refunds", sOpts());
  assert.deepEqual(instance.calls, [{ name: "search", args: [{ query: "refunds", ai_search_options: {} }] }]);
  assert.equal(got.tag, "Ok");
  assert.deepEqual(got.value[0], {
    id: "a",
    score: 0.8,
    text: "text a",
    key: "docs/a.md",
    timestamp: Some(1735689600000),
    metadata: Some('{"lang":"en"}'),
  });
});

t("WorkersAiSearch.search: sends each option the author set, with query_rewrite and reranking beside retrieval", async () => {
  const instance = fakeSearch();
  await as(instance).search(
    "q",
    sOpts({
      maxResults: Some(5),
      matchThreshold: Some(0.6),
      retrieval: Some({ tag: "Keyword" }),
      rewriteQuery: true,
      rerank: true,
      filter: [eq("folder", metaText("docs/")), range("timestamp", "FilterGte", 10), range("timestamp", "FilterLt", 20)],
    }),
  );
  assert.deepEqual(instance.calls[0].args[0].ai_search_options, {
    retrieval: {
      max_num_results: 5,
      match_threshold: 0.6,
      retrieval_type: "keyword",
      filters: { folder: { $eq: "docs/" }, timestamp: { $gte: 10, $lt: 20 } },
    },
    query_rewrite: { enabled: true },
    reranking: { enabled: true },
  });
});

t("WorkersAiSearch.search: an out-of-range option or an unsendable filter is SearchInvalidInput, and nothing is sent", async () => {
  const instance = fakeSearch();
  const refused = async (over) => (await as(instance).search("q", sOpts(over))).error;
  for (const over of [
    { maxResults: Some(0) },
    { maxResults: Some(51) },
    { matchThreshold: Some(1.5) },
    { matchThreshold: Some(-0.1) },
    { filter: [eq("", metaText("x"))] },
    { filter: [eq("$and", metaText("x"))] },
    { filter: [eq("k".repeat(513), metaText("x"))] },
    { filter: [eq("f", { tag: "MetaFlag", value: true }), eq("f", metaText("x"))] },
    { filter: [range("n", "FilterGt", 1), range("n", "FilterGte", 2)] },
    { filter: [{ field: "f", op: { tag: "FilterLt", value: { tag: "MetaFlag", value: true } } }] },
  ]) {
    assert.equal((await refused(over)).tag, "SearchInvalidInput");
  }
  assert.equal(instance.calls.length, 0);
});

t("WorkersAiSearch.search: a filter's every operator takes the platform's $ spelling", async () => {
  const instance = fakeSearch();
  const op = (tag, value) => ({ field: "k", op: Array.isArray(value) ? { tag, values: value } : { tag, value } });
  const sent = async (condition) => {
    instance.calls.length = 0;
    await as(instance).search("q", sOpts({ filter: [condition] }));
    return instance.calls[0].args[0].ai_search_options.retrieval.filters.k;
  };
  assert.deepEqual(await sent(op("FilterNe", metaText("x"))), { $ne: "x" });
  assert.deepEqual(await sent(op("FilterAnyOf", [metaText("a"), metaText("b")])), { $in: ["a", "b"] });
  assert.deepEqual(await sent(op("FilterNoneOf", [metaNum(1)])), { $nin: [1] });
  assert.deepEqual(await sent(op("FilterEq", { tag: "MetaFlag", value: true })), { $eq: true });
});

t("WorkersAiSearch.search: a query against an empty index is an empty list", async () => {
  assert.deepEqual(await as(fakeSearch({ search: { chunks: [] } })).search("q", sOpts()), { tag: "Ok", value: [] });
});

t("WorkersAiSearch.search: a reply that is not the documented shape is SearchFailed, never a fault", async () => {
  for (const reply of [42, "text", {}, { chunks: "no" }, { chunks: [{ id: "a" }] }, { chunks: [chunkOf("a", { item: {} })] }]) {
    const got = await as(fakeSearch({ search: reply })).search("q", sOpts());
    assert.equal(got.error.tag, "SearchFailed");
    assert.match(got.error.message, /^unexpected reply from AI Search: /);
  }
});

t("WorkersAiSearch.search: an item without a timestamp or metadata gives None", async () => {
  const bare = chunkOf("a", { item: { key: "docs/a.md" } });
  const got = await as(fakeSearch({ search: { chunks: [bare] } })).search("q", sOpts());
  assert.deepEqual(got.value[0], { id: "a", score: 0.8, text: "text a", key: "docs/a.md", timestamp: None, metadata: None });
});

t("WorkersAiSearch.search: the platform's refusals map by message, and anything else is SearchFailed", async () => {
  const failing = (message) => as(fakeSearch({ search: new Error(message) })).search("q", sOpts());
  assert.equal((await failing("Rate limit exceeded")).error.tag, "SearchRateLimited");
  assert.equal((await failing("429 Too Many Requests")).error.tag, "SearchRateLimited");
  assert.equal((await failing("You have exceeded your usage quota")).error.tag, "SearchQuotaExceeded");
  assert.deepEqual((await failing("invalid filter: unknown operator")).error, {
    tag: "SearchInvalidInput",
    message: "invalid filter: unknown operator",
  });
  assert.deepEqual((await failing("something new")).error, { tag: "SearchFailed", message: "something new" });
});

t("WorkersAiSearch.search: a message about something else of the instance stays a value, not a fault", async () => {
  const failing = (message) => as(fakeSearch({ search: new Error(message) })).search("q", sOpts());
  for (const message of [
    "The generation model configured for instance support-docs was not found",
    "instance support-docs: source bucket not found",
  ]) {
    assert.deepEqual((await failing(message)).error, { tag: "SearchFailed", message });
  }
  // An auth or upstream problem is not a malformed request.
  for (const message of ["Invalid API token", "invalid binding", "invalid response from upstream"]) {
    assert.equal((await failing(message)).error.tag, "SearchFailed", message);
  }
  assert.equal((await failing("invalid filter: unknown operator")).error.tag, "SearchInvalidInput");
});

t("WorkersAiSearch.search: an instance that is not there is a fault, as is a missing binding", async () => {
  await assert.rejects(as(fakeSearch({ search: new Error("Instance support-docs not found") })).search("q", sOpts()), /not found/);
  await assert.rejects(as(fakeSearch({ search: new Error("instance `support-docs` does not exist") })).search("q", sOpts()), /does not exist/);
  await assert.rejects(as(fakeSearch({ search: new Error("No such instance") })).search("q", sOpts()), /No such instance/);
  await assert.rejects(new WorkersAiSearch({}, "AI_SEARCH_KB").search("q", sOpts()), /requires an AI Search binding/);
});

t("WorkersAiSearch.answer: sends the conversation and returns the text, the chunks and the usage", async () => {
  const instance = fakeSearch();
  const got = await as(instance).answer([{ role: { tag: "System" }, content: "Be brief." }, userMsg("refunds?")], sOpts({ maxResults: Some(3) }));
  assert.deepEqual(instance.calls[0].args[0], {
    messages: [
      { role: "system", content: "Be brief." },
      { role: "user", content: "refunds?" },
    ],
    ai_search_options: { retrieval: { max_num_results: 3 } },
  });
  assert.equal(got.tag, "Ok");
  assert.equal(got.value.text, "Within 30 days.");
  assert.equal(got.value.hits.length, 1);
  assert.deepEqual(got.value.usage, Some({ promptTokens: 12, completionTokens: 5 }));
});

t("WorkersAiSearch.answer: a reply with no usage or chunks still answers", async () => {
  const reply = { choices: [{ message: { role: "assistant", content: "ok" } }] };
  assert.deepEqual(await as(fakeSearch({ chatCompletions: reply })).answer([userMsg("q")], sOpts()), {
    tag: "Ok",
    value: { text: "ok", hits: [], usage: None },
  });
});

t("WorkersAiSearch.answer: a reply that is not the documented shape is SearchFailed, never a fault", async () => {
  for (const reply of [42, {}, { choices: [] }, { choices: [{ message: { content: 7 } }] }, { choices: [{ message: { content: "x" } }], chunks: "no" }]) {
    const got = await as(fakeSearch({ chatCompletions: reply })).answer([userMsg("q")], sOpts());
    assert.equal(got.error.tag, "SearchFailed");
    assert.match(got.error.message, /^unexpected reply from AI Search: /);
  }
});

t("WorkersAiSearch.answer: an empty conversation is a fault, and a bad option is refused before the call", async () => {
  const instance = fakeSearch();
  await assert.rejects(as(instance).answer([], sOpts()), /needs at least one message/);
  assert.equal((await as(instance).answer([userMsg("q")], sOpts({ maxResults: Some(99) }))).error.tag, "SearchInvalidInput");
  assert.equal(instance.calls.length, 0);
});

t("WorkersAiSearch.answer: the platform's refusals map as for search", async () => {
  const failing = (message) => as(fakeSearch({ chatCompletions: new Error(message) })).answer([userMsg("q")], sOpts());
  assert.equal((await failing("rate limited")).error.tag, "SearchRateLimited");
  assert.equal((await failing("boom")).error.tag, "SearchFailed");
});

// #3479: `bynk dev` binds a Vectorize to an index of the developer's own.
t("WorkersVectorize: under bynk dev, warns once unless acknowledged, and never in a deployment", async () => {
  const warnings = [];
  const original = console.warn;
  console.warn = (...args) => warnings.push(args.join(" "));
  try {
    const run = async (env) => new WorkersVectorize({ VECTORIZE_DOCS: fakeIndex(), ...env }, "VECTORIZE_DOCS").describe();
    assert.equal((await run({})).tag, "Ok");
    assert.equal((await run({ BYNK_DEV: "1", BYNK_DEV_ACK_VECTORIZE: "allow" })).tag, "Ok");
    assert.deepEqual(warnings, []);
    assert.equal((await run({ BYNK_DEV: "1" })).tag, "Ok");
    assert.equal(warnings.length, 1);
    assert.match(warnings[0], /may incur charges/);
    assert.match(warnings[0], /https:\/\/developers\.cloudflare\.com\/vectorize\/platform\/pricing\//);
    assert.match(warnings[0], /vectorize = "allow"/);
    assert.doesNotMatch(warnings[0], /\$|free/i);
    assert.equal((await run({ BYNK_DEV: "1" })).tag, "Ok");
    assert.equal(warnings.length, 1);
  } finally {
    console.warn = original;
  }
});

t("WorkersVectorize: an instance with no development index faults, saying how to bind one", async () => {
  const unbound = new WorkersVectorize({ BYNK_DEV: "1" }, "VECTORIZE_MY_DOCS");
  await assert.rejects(() => unbound.describe(), /`MyDocs` has no development index: set `\[dev\.vectorize-index\] MyDocs = "<index-name>"`/);
});

t("WorkersVectorize: the first call under bynk dev checks the index against the declaration", async () => {
  const env = (index, declared) => ({ VECTORIZE_DOCS: index, BYNK_DEV: "1", BYNK_DEV_ACK_VECTORIZE: "allow", BYNK_DEV_INDEX_VECTORIZE_DOCS: declared });
  // A wrong index faults, on every call until it is right.
  const wrong = fakeIndex({ describe: { dimensions: 8, metric: "cosine" } });
  const bad = new WorkersVectorize(env(wrong, "3:cosine"), "VECTORIZE_DOCS");
  await assert.rejects(() => bad.query([0, 0, 0], opts()), /has 8 dimensions and the cosine metric, but the declaration says 3 and cosine/);
  await assert.rejects(() => bad.upsert([vec("a")]), /declaration says 3 and cosine/);
  assert.equal(wrong.calls.filter((c) => c.name === "query" || c.name === "upsert").length, 0);
  // An index that cannot be reached faults, naming that bynk dev never creates one.
  const gone = fakeIndex({ describe: new Error("index not found") });
  await assert.rejects(() => new WorkersVectorize(env(gone, "3:cosine"), "VECTORIZE_DOCS").getByIds(["a"]), /could not reach the development index.*never creates one/);
  // A matching index is checked once, then the call goes ahead.
  const good = fakeIndex({ describe: { config: { dimensions: 3, metric: "cosine" } } });
  const ok = new WorkersVectorize(env(good, "3:cosine"), "VECTORIZE_DOCS");
  assert.equal((await ok.upsert([vec("a")])).tag, "Ok");
  assert.equal((await ok.deleteByIds(["a"])).tag, "Ok");
  assert.equal(good.calls.filter((c) => c.name === "describe").length, 1);
  // The check's own describe is reused by a first `describe()` call: one platform call, not two.
  const once = fakeIndex({ describe: { dimensions: 3, metric: "cosine" } });
  const first = new WorkersVectorize({ ...env(once, "3:cosine"), BYNK_DEV_INDEX_VECTORIZE_DOCS: "3:cosine" }, "VECTORIZE_DOCS");
  assert.deepEqual((await first.describe()).value, { dimensions: 3, metric: "cosine" });
  assert.equal(once.calls.filter((c) => c.name === "describe").length, 1);
  // Not under bynk dev (a deployment, or bynk test): no check at all.
  const plain = fakeIndex({ describe: { dimensions: 99, metric: "euclidean" } });
  assert.equal((await new WorkersVectorize({ VECTORIZE_DOCS: plain }, "VECTORIZE_DOCS").upsert([vec("a")])).tag, "Ok");
  assert.equal(plain.calls.filter((c) => c.name === "describe").length, 0);
});

// #3480: AI Search is a remote-only binding of its own kind: its warning is
// keyed `ai-search`, acknowledged by its own variable (not Workers AI's), once
// per session, never in a deployment, and links to Cloudflare, not a price.
t("WorkersAiSearch.search: warns once under bynk dev unless the project acknowledged ai-search, and never in a deployment", async () => {
  const warnings = [];
  const original = console.warn;
  console.warn = (...args) => warnings.push(args.join(" "));
  try {
    const run = async (env) => new WorkersAiSearch({ AI_SEARCH_KB: fakeSearch(), ...env }, "AI_SEARCH_KB").search("q", sOpts());
    assert.equal((await run({})).tag, "Ok");
    assert.deepEqual(warnings, []);
    // Workers AI's acknowledgement does not cover AI Search.
    assert.equal((await run({ BYNK_DEV: "1", BYNK_DEV_ACK_AI: "allow" })).tag, "Ok");
    assert.equal(warnings.length, 1);
    assert.match(warnings[0], /may incur charges/);
    assert.match(warnings[0], /https:\/\/developers\.cloudflare\.com\/ai-search\//);
    assert.match(warnings[0], /ai-search = "allow"/);
    assert.doesNotMatch(warnings[0], /\$|free/i);
    // Once per session: a second call, even with no acknowledgement, is quiet.
    assert.equal((await run({ BYNK_DEV: "1" })).tag, "Ok");
    assert.equal(warnings.length, 1);
  } finally {
    console.warn = original;
  }
});

t("WorkersAiSearch.answer: an acknowledged project is never warned", async () => {
  const warnings = [];
  const original = console.warn;
  console.warn = (...args) => warnings.push(args.join(" "));
  try {
    const instance = new WorkersAiSearch({ AI_SEARCH_KB: fakeSearch(), BYNK_DEV: "1", BYNK_DEV_ACK_AI_SEARCH: "allow" }, "AI_SEARCH_KB");
    assert.equal((await instance.answer([userMsg("q")], sOpts())).tag, "Ok");
    assert.deepEqual(warnings, []);
  } finally {
    console.warn = original;
  }
});

// #3477: `WorkersContainer`, against a fake of the Durable Object namespace the
// generated class sits behind. The class itself (starting the container) is
// tested in the runtime; here only what the provider sends and how it reads the
// class's answer.
function fakeContainerNs(answer) {
  const seen = [];
  return {
    seen,
    idFromName: (name) => `id:${name}`,
    get: (id) => ({
      fetch: async (request) => {
        seen.push({ id, url: request.url, method: request.method, body: await request.text() });
        return answer(request);
      },
    }),
  };
}
const creq = (over = {}) => ({ method: { tag: "Post" }, url: "http://renderer/render", contentType: Some("text/plain"), authorization: None, headers: new Map(), body: Some("hello"), ...over });
const cont = (ns) => new WorkersContainer({ CONTAINER_RENDERER: ns }, "CONTAINER_RENDERER");

t("WorkersContainer.send: reaches the named instance and returns the container's response", async () => {
  const ns = fakeContainerNs(() => new Response("<svg/>", { status: 201 }));
  const r = await cont(ns).send("tenant-a", creq());
  assert.equal(r.tag, "Ok");
  assert.equal(r.value.status, 201);
  assert.equal(r.value.body, "<svg/>");
  assert.deepEqual(ns.seen, [{ id: "id:tenant-a", url: "http://renderer/render", method: "POST", body: "hello" }]);
});

t("WorkersContainer.receiveBytes and sendBytes: bytes both ways", async () => {
  const ns = fakeContainerNs(() => new Response(new Uint8Array([1, 2, 3])));
  const got = await cont(ns).receiveBytes("a", creq({ method: { tag: "Get" }, body: None }));
  assert.deepEqual([...got.value.body], [1, 2, 3]);
  const sent = await cont(ns).sendBytes("a", creq({ body: None }), new Uint8Array([9, 8]));
  assert.equal(sent.tag, "Ok");
  assert.equal(ns.seen[1].body, "\u0009\u0008");
});

t("WorkersContainer: a 5xx from the container is its answer, not an error", async () => {
  const r = await cont(fakeContainerNs(() => new Response("boom", { status: 503 }))).send("a", creq());
  assert.equal(r.tag, "Ok");
  assert.equal(r.value.status, 503);
});

t("WorkersContainer: an error the generated class reports is a ContainerError value", async () => {
  const reply = (kind) => () => new Response("the port never listened", { status: 502, headers: { "bynk-container-error": kind } });
  assert.deepEqual((await cont(fakeContainerNs(reply("start-failed"))).send("a", creq())).error, { tag: "StartFailed", message: "the port never listened" });
  assert.deepEqual((await cont(fakeContainerNs(reply("unavailable"))).send("a", creq())).error, { tag: "Unavailable" });
  assert.deepEqual((await cont(fakeContainerNs(reply("timeout"))).send("a", creq())).error, { tag: "Timeout" });
  assert.deepEqual((await cont(fakeContainerNs(reply("other"))).send("a", creq())).error, { tag: "Network" });
});

t("WorkersContainer: a failed call is Network", async () => {
  const r = await cont(fakeContainerNs(() => { throw new Error("socket closed"); })).send("a", creq());
  assert.deepEqual(r.error, { tag: "Network" });
});

t("WorkersContainer: a bad instance name, a refused request or a missing binding faults", async () => {
  const ns = fakeContainerNs(() => new Response("x"));
  for (const bad of ["", "a".repeat(257), "a\nb", "a\u0000b"]) {
    await assert.rejects(() => cont(ns).send(bad, creq()), /an instance name is 1 to 256 bytes/);
  }
  await assert.rejects(() => cont(ns).send("a", creq({ headers: new Map([["host", "x"]]) })), /not one the platform can send/);
  await assert.rejects(() => cont(ns).send("a", creq({ method: { tag: "Get" } })), /not one the platform can send/);
  await assert.rejects(() => new WorkersContainer({}, "CONTAINER_RENDERER").send("a", creq()), /\[\[containers\]\]/);
  assert.equal(ns.seen.length, 0);
});

t("WorkersContainer: a response cannot forge a ContainerError without the header, and the header is the class's", async () => {
  const r = await cont(fakeContainerNs(() => new Response("StartFailed", { status: 500 }))).send("a", creq());
  assert.equal(r.tag, "Ok");
});
