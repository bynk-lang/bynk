import { test } from "node:test";
import assert from "node:assert/strict";
import {
  BynkInspectRegistry,
  HISTORY_KEY,
  HISTORY_LIMIT,
  historyBefore,
  historyRecord,
  INSPECT_CALLER,
  inspectAgent,
  inspectRoute,
  noteAgentInstance,
  type InspectInfo,
} from "../src/inspect.ts";
import { signedHeaders } from "../src/boundary.ts";
import { makeTestState } from "../src/storage.ts";
import { mapKey, metaKey } from "../src/entries.ts";

const info: InspectInfo = { agent: "Cart", current: 2, oldestReadable: 1, steps: [] };
const fields = [{ name: "lines", kind: "map" }] as const;

async function read(state: ReturnType<typeof makeTestState>, query = "", i: InspectInfo = info) {
  const url = new URL(`https://_bynk/_bynk/inspect${query}`);
  const response = await inspectAgent(state, fields, i, url);
  return (await response.json()) as Record<string, any>;
}

test("an instance with no stored state is reported as absent", async () => {
  assert.deepEqual(await read(makeTestState("a")), { class: "Cart", exists: false });
});

test("a layout-2 record returns its cells and pages its collections, without writing", async () => {
  const state = makeTestState("a");
  await state.storage.put("state", { total: 3, __v: 2, __layout: 2 });
  for (const k of ["a", "b", "c"]) await state.storage.put(mapKey("lines", k), { i: 0, v: k.toUpperCase() });
  await state.storage.put(metaKey("lines"), { size: 3, seq: 3 });
  const before = [...(await state.storage.list({})).entries()];

  const first = await read(state, "?limit=2");
  assert.equal(first.readable, true);
  assert.equal(first.version, 2);
  assert.equal(first.layout, 2);
  assert.deepEqual(first.cells, { total: 3 });
  assert.equal(first.collections.lines.size, 3);
  assert.deepEqual(first.collections.lines.entries, [
    { key: "a", value: { i: 0, v: "A" } },
    { key: "b", value: { i: 0, v: "B" } },
  ]);
  const next = first.collections.lines.next as string;
  const second = await read(state, `?field=lines&limit=2&after=${encodeURIComponent(next)}`);
  assert.deepEqual(second.collections.lines.entries, [{ key: "c", value: { i: 0, v: "C" } }]);
  assert.equal(second.collections.lines.next, null);

  assert.deepEqual([...(await state.storage.list({})).entries()], before, "a read writes nothing");
});

test("a wholesale record is paged from the record itself", async () => {
  const state = makeTestState("a");
  await state.storage.put("state", { total: 1, lines: { a: 1, b: 2, c: 3 }, __v: 2 });
  const page = await read(state, "?limit=2");
  assert.equal(page.layout, 1);
  assert.equal(page.collections.lines.size, 3);
  assert.equal(page.collections.lines.next, "2");
  assert.deepEqual(page.cells, { total: 1 });
});

test("a cursor that is not a position pages from the start", async () => {
  const state = makeTestState("a");
  await state.storage.put("state", { lines: { a: 1, b: 2 }, __v: 2 });
  for (const after of ["-3", "x", "1.5"]) {
    const page = await read(state, `?field=lines&after=${after}&limit=5`);
    assert.equal(page.collections.lines.entries.length, 2, after);
  }
});

test("a record from a later build is reported unreadable, not decoded", async () => {
  const newer = makeTestState("a");
  await newer.storage.put("state", { __v: 2, __layout: 9 });
  assert.deepEqual(await read(newer), {
    class: "Cart",
    exists: true,
    version: 2,
    layout: 9,
    current: 2,
    readable: false,
    fault: "NewerLayout",
  });
  const future = makeTestState("b");
  await future.storage.put("state", { __v: 5, __layout: 2 });
  assert.equal((await read(future)).fault, "NewerSchema");
});

test("a record behind the build with a covering step is flagged as pending migration, and left as it is", async () => {
  const state = makeTestState("a");
  await state.storage.put("state", { total: 1, __v: 1, __layout: 2 });
  const out = await read(state, "", { ...info, current: 3, oldestReadable: 2, steps: [[1, 1]] });
  assert.equal(out.readable, true);
  assert.equal(out.migrationPending, true);
  assert.deepEqual(await state.storage.get("state"), { total: 1, __v: 1, __layout: 2 });
});

// A registry namespace over one in-memory registry object.
function registryNamespace() {
  const registry = new BynkInspectRegistry(makeTestState("registry"));
  return {
    idFromName: (n: string) => n,
    get: () => ({ fetch: (u: string, init?: RequestInit) => registry.fetch(new Request(u, init)) }),
  };
}

const codec = { enc: (v: string) => v, dec: (j: unknown) => ({ tag: "Ok" as const, value: j as string }) };

test("an addressed instance is listed once, by class, key and id", async () => {
  const registry = registryNamespace();
  const binding = { idFromName: (n: string) => `id-${n}`, get: () => ({ fetch: async () => new Response("") }) };
  const agent = { tag: "agent" };
  assert.equal(noteAgentInstance(registry, binding, "Cart", "u1", codec as never, agent), agent);
  noteAgentInstance(registry, binding, "Cart", "u1", codec as never, agent);
  noteAgentInstance(registry, binding, "Cart", "u2", codec as never, agent);
  await new Promise((r) => setTimeout(r, 10));
  const listed = (await (await registry.get().fetch("https://_bynk/_bynk/inspect/list?class=Cart")).json()) as {
    instances: Array<{ key: string }>;
  };
  assert.deepEqual(listed.instances.map((i) => i.key).sort(), ["u1", "u2"]);
});

test("recording is best effort: no registry, or a key that cannot encode, changes nothing", () => {
  const agent = {};
  assert.equal(noteAgentInstance(undefined, undefined, "Cart", "k", codec as never, agent), agent);
  const throwing = { enc: () => { throw new Error("no wire form"); }, dec: codec.dec };
  const binding = { idFromName: (n: string) => n, get: () => ({ fetch: async () => new Response("") }) };
  assert.equal(noteAgentInstance(registryNamespace(), binding, "Cart", "k", throwing as never, agent), agent);
});

test("the route refuses a request that is not signed, and answers a signed one", async () => {
  const env = { BYNK_CALL_FROM_BYNK__DEV: "local-key", __BYNK_INSPECT_REGISTRY: registryNamespace() };
  const bindings = { Cart: "CART" };
  const unsigned = await inspectRoute(env, new Request("http://w/_bynk/inspect/agents", { method: "POST" }), "shop", bindings);
  assert.equal(unsigned.status, 401);
  assert.equal(await unsigned.text(), "Unauthorized");

  const path = "/_bynk/inspect/agents";
  const keyEnv = { ...env, BYNK_CALL_TO_SHOP: "local-key" };
  const headers = await signedHeaders(keyEnv, INSPECT_CALLER, "shop", path, "", "1", "", "inspect");
  const signed = await inspectRoute(env, new Request(`http://w${path}`, { method: "POST", headers }), "shop", bindings);
  assert.equal(signed.status, 200);
  assert.deepEqual(await signed.json(), { context: "shop", instances: [] });

  // A sibling context holding an edge key to this Worker signs validly as itself,
  // and is still refused: only `bynk.dev` is admitted.
  const sibling = { ...env, BYNK_CALL_FROM_BILLING: "billing-key", BYNK_CALL_TO_SHOP: "billing-key" };
  const billing = await signedHeaders(sibling, "billing", "shop", path, "", "1", "", "inspect");
  const refused = await inspectRoute(sibling, new Request(`http://w${path}`, { method: "POST", headers: billing }), "shop", bindings);
  assert.equal(refused.status, 401);

  const get = await inspectRoute(env, new Request(`http://w${path}`), "shop", bindings);
  assert.equal(get.status, 405);
});

// A commit as a dev build's `commitState` makes it: snapshot, write, record.
async function commit(
  state: ReturnType<typeof makeTestState>,
  handler: string | undefined,
  next: Record<string, unknown>,
  lines: number,
) {
  const before = await historyBefore(state.storage, fields);
  await state.storage.put("state", { ...next, __v: 2, __layout: 2 });
  await state.storage.put(metaKey("lines"), { size: lines, seq: lines });
  await historyRecord(state.storage, fields, before, handler);
}

test("each commit appends {handler, old, new, at}, and a read returns them in order", async () => {
  const state = makeTestState("a");
  await commit(state, "add", { total: 1 }, 1);
  await commit(state, "add", { total: 3 }, 2);
  const out = await read(state);
  assert.equal(out.history.length, 2);
  assert.equal(out.history[0].handler, "add");
  assert.equal(out.history[0].old, null, "the first commit replaced nothing");
  assert.deepEqual(out.history[0].new, { cells: { total: 1 }, sizes: { lines: 1 } });
  assert.deepEqual(out.history[1].old, { cells: { total: 1 }, sizes: { lines: 1 } });
  assert.deepEqual(out.history[1].new, { cells: { total: 3 }, sizes: { lines: 2 } });
  assert.equal(typeof out.history[1].at, "number");
});

test("the buffer is bounded, drops the oldest, and lives outside layout 2's keys", async () => {
  const state = makeTestState("a");
  for (let i = 0; i < HISTORY_LIMIT + 5; i++) await commit(state, `h${i}`, { total: i }, 0);
  const kept = (await state.storage.get(HISTORY_KEY)) as Array<{ handler: string }>;
  assert.equal(kept.length, HISTORY_LIMIT);
  assert.equal(kept[0]!.handler, "h5");
  assert.equal(kept[kept.length - 1]!.handler, `h${HISTORY_LIMIT + 4}`);
  for (const prefix of ["m/", "s/", "l/", "i/", "x/", "c/"]) assert.ok(!HISTORY_KEY.startsWith(prefix));
  // A read of the cells and collections does not see it as one.
  const out = await read(state);
  assert.deepEqual(Object.keys(out.cells), ["total"]);
});

test("a commit with no handler name is recorded as null, and recording never throws", async () => {
  const state = makeTestState("a");
  await commit(state, undefined, { total: 1 }, 0);
  assert.equal(((await state.storage.get(HISTORY_KEY)) as Array<{ handler: unknown }>)[0]!.handler, null);
  const broken = { get: async () => { throw new Error("boom"); }, put: async () => { throw new Error("boom"); } };
  assert.equal(await historyBefore(broken as never, fields), undefined);
  await historyRecord(broken as never, fields, null, "x");
});
