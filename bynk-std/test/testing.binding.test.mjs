// `testing.binding`: `bynk.testing`'s in-memory typed-read store (#3458). A
// typed read returns the stored JSON as it is: the compiler decodes the reply.

import { test } from "node:test";
import assert from "node:assert/strict";
import { MemoryTypedReads } from "./bynk/testing.binding.js";

test("testing.binding MemoryTypedReads.put: stores the JSON text as it is", async () => {
  const s = new MemoryTypedReads();
  await s.put("k", '{"a":1}');
  assert.deepEqual(await s.read("k"), { a: 1 });
});

test("testing.binding MemoryTypedReads.write: stores a copy of the encoding it is given", async () => {
  const s = new MemoryTypedReads();
  const v = { id: "a", total: 5 };
  await s.write("k", v);
  v.total = 6;
  assert.deepEqual(await s.read("k"), { id: "a", total: 5 });
});

test("testing.binding MemoryTypedReads.read: a missing key is null", async () => {
  assert.equal(await new MemoryTypedReads().read("nobody"), null);
});

test("testing.binding MemoryTypedReads.readOpt: a missing key is null and a stored value is returned", async () => {
  const s = new MemoryTypedReads();
  assert.equal(await s.readOpt("nobody"), null);
  await s.write("k", { kind: "Some", value: 1 });
  assert.deepEqual(await s.readOpt("k"), { kind: "Some", value: 1 });
});
