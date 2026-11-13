// #3479: the in-memory Vectorize index tests run against. Its answers follow
// Cloudflare's documented contract (scores per metric, insert versus upsert,
// namespaces, filters, limits); whether production agrees is not something a
// test here can show.

import { test } from "node:test";
import assert from "node:assert/strict";
import { VECTORIZE_LIMITS, makeIntegrationVectorize } from "../src/vectorize.ts";

const ids = (r: unknown): string[] => (r as { matches: { id: string }[] }).matches.map((m) => m.id);

test("the platform limits the double enforces are pinned, with the date they were read", () => {
  assert.deepEqual(VECTORIZE_LIMITS, {
    readOn: "2026-11-13",
    topK: 100,
    topKWithValuesOrAllMetadata: 50,
    upsertBatch: 1000,
    metadataBytes: 10240,
    idBytes: 64,
    namespaceBytes: 64,
    filterBytes: 2048,
    filterKeyChars: 512,
  });
});

test("scores follow the metric: cosine larger is nearer, euclidean smaller, dot-product the negated product", async () => {
  const data = [
    { id: "near", values: [1, 0] },
    { id: "far", values: [0, 1] },
  ];
  const cosine = makeIntegrationVectorize(2, "cosine");
  await cosine.upsert(data);
  assert.deepEqual(ids(await cosine.query([1, 0], {})), ["near", "far"]);
  assert.equal(((await cosine.query([1, 0], {})) as { matches: { score: number }[] }).matches[0].score, 1);

  const euclid = makeIntegrationVectorize(2, "euclidean");
  await euclid.upsert([{ id: "a", values: [3, 4] }, { id: "b", values: [0, 0] }]);
  const e = (await euclid.query([0, 0], {})) as { matches: { id: string; score: number }[] };
  assert.deepEqual(e.matches.map((m) => [m.id, m.score]), [["b", 0], ["a", 5]]);

  const dot = makeIntegrationVectorize(2, "dot-product");
  await dot.upsert([{ id: "big", values: [10, 0] }, { id: "small", values: [1, 0] }]);
  const d = (await dot.query([1, 0], {})) as { matches: { id: string; score: number }[] };
  assert.deepEqual(d.matches.map((m) => [m.id, m.score]), [["big", -10], ["small", -1]]);
});

test("mutation ids are deterministic and a write is visible at once", async () => {
  const index = makeIntegrationVectorize(1, "euclidean");
  assert.deepEqual(await index.upsert([{ id: "a", values: [1] }]), { mutationId: "m1" });
  assert.deepEqual(await index.insert([{ id: "b", values: [2] }]), { mutationId: "m2" });
  assert.deepEqual(await index.deleteByIds(["a"]), { mutationId: "m3" });
  assert.deepEqual(ids(await index.query([0], {})), ["b"]);
});

test("insert keeps an existing id and upsert replaces in full", async () => {
  const index = makeIntegrationVectorize(1, "euclidean", ["k:string"]);
  await index.upsert([{ id: "a", values: [1], metadata: { k: "x" } }]);
  await index.insert([{ id: "a", values: [9] }]);
  assert.deepEqual(await index.getByIds(["a"]), [{ id: "a", values: [1], namespace: undefined, metadata: { k: "x" } }]);
  await index.upsert([{ id: "a", values: [2] }]);
  assert.deepEqual(await index.getByIds(["a"]), [{ id: "a", values: [2], namespace: undefined, metadata: undefined }]);
});

test("a wrong-length vector and the platform limits are refused", async () => {
  const index = makeIntegrationVectorize(2, "cosine");
  await assert.rejects(() => index.upsert([{ id: "a", values: [1] }]), /1 dimensions, but the index has 2/);
  await assert.rejects(() => index.query([1], {}), /dimensions/);
  await assert.rejects(() => index.query([1, 0], { topK: 101 }), /topK must be between 1 and 100/);
  await assert.rejects(() => index.query([1, 0], { topK: 51, returnValues: true }), /limited to a maximum of 50/);
  await assert.rejects(() => index.upsert(Array.from({ length: 1001 }, (_, i) => ({ id: `v${i}`, values: [0, 0] }))), /maximum of 1000/);
  await assert.rejects(() => index.upsert([{ id: "x".repeat(65), values: [0, 0] }]), /1 to 64 bytes/);
  await assert.rejects(() => index.upsert([{ id: "a", values: [0, 0], metadata: { big: "x".repeat(10241) } }]), /metadata/);
});

test("filters: operators, dotted keys, namespaces, and the indexed-only metadata scope", async () => {
  const index = makeIntegrationVectorize(1, "euclidean", ["category:string", "year:number", "author.name:string", "live:boolean"]);
  await index.upsert([
    { id: "a", values: [1], metadata: { category: "faq", year: 2024, author: { name: "ann" }, live: true, note: "n" } },
    { id: "b", values: [2], metadata: { category: "blog", year: 2026, author: { name: "bob" }, live: false } },
    { id: "c", values: [3], metadata: { category: "faq", year: 2026, author: { name: "ann" }, live: true }, namespace: "ns" },
  ]);
  const q = async (filter: Record<string, unknown>, extra: Record<string, unknown> = {}) =>
    ids(await index.query([0], { filter, ...extra }));
  assert.deepEqual(await q({ category: "faq" }), ["a"]);
  assert.deepEqual(await q({ category: { $ne: "faq" } }), ["b"]);
  assert.deepEqual(await q({ category: { $in: ["faq", "blog"] } }), ["a", "b"]);
  assert.deepEqual(await q({ category: { $nin: ["faq"] } }), ["b"]);
  assert.deepEqual(await q({ year: { $gte: 2025 } }), ["b"]);
  assert.deepEqual(await q({ year: { $gt: 2023, $lt: 2025 } }), ["a"]);
  assert.deepEqual(await q({ "author.name": "ann" }), ["a"]);
  assert.deepEqual(await q({ live: false }), ["b"]);
  assert.deepEqual(await q({ category: "faq" }, { namespace: "ns" }), ["c"]);
  const scoped = (await index.query([0], { returnMetadata: "indexed" })) as { matches: { metadata: Record<string, unknown> }[] };
  assert.equal("note" in scoped.matches[0].metadata, false);
  const all = (await index.query([0], { returnMetadata: "all" })) as { matches: { metadata: Record<string, unknown> }[] };
  assert.equal(all.matches[0].metadata.note, "n");
});

test("a filter on a property that is not indexed faults, naming the difference; bad filters are refused", async () => {
  const index = makeIntegrationVectorize(1, "euclidean", ["category:string"]);
  await assert.rejects(() => index.query([0], { filter: { note: "x" } }), /^Error: bynk: the Vectorize test double does not model a filter on `note`/);
  await assert.rejects(() => index.query([0], { filter: {} }), /non-empty/);
  await assert.rejects(() => index.query([0], { filter: { "$bad": 1 } }), /invalid filter key/);
});
