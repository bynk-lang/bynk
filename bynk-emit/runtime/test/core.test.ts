import { test } from "node:test";
import assert from "node:assert/strict";
import { Ok, Err, Some, None } from "../src/result.ts";
import { QueueResult } from "../src/queue.ts";
import { InMemoryStorage, makeTestState } from "../src/storage.ts";
import { fault } from "../src/errors.ts";

test("Result/Option constructors carry the tag discriminant", () => {
  assert.deepEqual(Ok(1), { tag: "Ok", value: 1 });
  assert.deepEqual(Err("e"), { tag: "Err", error: "e" });
  assert.deepEqual(Some(1), { tag: "Some", value: 1 });
  assert.deepEqual(None, { tag: "None" });
});

test("QueueResult: Ack is a singleton verdict, Retry carries a reason", () => {
  assert.deepEqual(QueueResult.Ack, { tag: "Ack" });
  assert.deepEqual(QueueResult.Retry("backoff"), { tag: "Retry", reason: "backoff" });
});

test("InMemoryStorage: get/put/delete and prefix list", async () => {
  const s = new InMemoryStorage();
  await s.put("user:1", { n: "a" });
  await s.put("user:2", { n: "b" });
  await s.put("order:1", { n: "c" });
  assert.deepEqual(await s.get("user:1"), { n: "a" });
  const users = await s.list({ prefix: "user:" });
  assert.equal(users.size, 2);
  assert.equal(await s.delete("user:1"), true);
  assert.equal(await s.get("user:1"), undefined);
});

// #1660: storage clones on the way in and out, as workerd's V8-serialised
// storage does. Mutating a value after `put`, or a value returned by `get`,
// must never reach what is stored.
test("InMemoryStorage: put and get clone, so stored state is never aliased", async () => {
  const s = new InMemoryStorage();
  const written = { items: { a: 1 }, bytes: new Uint8Array([1, 2]) };
  await s.put("state", written);
  written.items.a = 99;

  const read = await s.get<typeof written>("state");
  assert.ok(read !== undefined);
  assert.notEqual(read, written);
  assert.equal(read.items.a, 1);
  // The copy keeps the shapes workerd's storage keeps.
  assert.ok(read.bytes instanceof Uint8Array);

  read.items.a = 42;
  assert.equal((await s.get<typeof written>("state"))?.items.a, 1);

  const listed = await s.list<typeof written>();
  listed.get("state")!.items.a = 7;
  assert.equal((await s.get<typeof written>("state"))?.items.a, 1);
});

// #1660: a live handle (a held `TestConnection` on the bundle target) is shared,
// not copied. Workers re-resolve the same live socket from its stored connId, and
// copying would strip `send`. A value-level `Map` inside state is still copied.
test("InMemoryStorage: live handles are shared, data around them is copied", async () => {
  class Handle {
    sent: string[] = [];
    send(m: string): void { this.sent.push(m); }
  }
  const s = new InMemoryStorage();
  const conn = new Handle();
  await s.put("state", { conns: { alice: conn }, tags: new Map([["k", [1]]]) });

  const read = await s.get<{ conns: { alice: Handle }; tags: Map<string, number[]> }>("state");
  assert.ok(read !== undefined);
  assert.equal(read.conns.alice, conn);
  read.conns.alice.send("hi");
  assert.deepEqual(conn.sent, ["hi"]);

  read.tags.get("k")!.push(2);
  assert.deepEqual((await s.get<typeof read>("state"))?.tags.get("k"), [1]);
});

// #1660 review: a record decoded from JSON can carry an *own* `__proto__` key
// (`JSON.parse` defines it). The copy must keep it as an ordinary entry, not
// reassign the copy's prototype or drop it.
test("InMemoryStorage: an own __proto__ key is copied as an entry", async () => {
  const s = new InMemoryStorage();
  const items = JSON.parse('{"__proto__": {"polluted": true}, "a": 1}') as Record<string, unknown>;
  await s.put("state", { items });

  const read = await s.get<{ items: Record<string, unknown> }>("state");
  assert.ok(read !== undefined);
  assert.deepEqual(Object.keys(read.items).sort(), ["__proto__", "a"]);
  assert.equal(Object.getPrototypeOf(read.items), Object.prototype);
  assert.equal((read.items as { polluted?: boolean }).polluted, undefined);
  assert.deepEqual(Object.getOwnPropertyDescriptor(read.items, "__proto__")?.value, { polluted: true });
});

test("makeTestState: names the state and gives it fresh storage", async () => {
  const st = makeTestState("agent-7");
  assert.equal(st.id.name, "agent-7");
  await st.storage.put("k", 1);
  assert.equal(await st.storage.get("k"), 1);
});

test("fault: throws an error carrying its kind, message and location", () => {
  let caught: unknown;
  try {
    fault("Unreachable", "filtered upstream", "a.bynk:3:5");
  } catch (e) {
    caught = e;
  }
  assert.ok(caught instanceof Error);
  assert.equal(caught.message, "Unreachable: filtered upstream (a.bynk:3:5)");
  assert.deepEqual((caught as { fault?: unknown }).fault, {
    kind: "Unreachable",
    message: "filtered upstream",
    location: "a.bynk:3:5",
  });
});
