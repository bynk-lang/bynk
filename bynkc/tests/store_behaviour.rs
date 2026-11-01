//! Behavioural test for the storage-track `store`/`Cell` emission (ADR 0109).
//!
//! Snapshots (the e2e golden) prove the emitted *shape*; this proves the
//! *runtime semantics* on the generated code — the same properties the ADR 0109
//! spike validated on a hand-written Durable Object, now on the emitter's output:
//!
//!   - a `:=` write persists across handler invocations;
//!   - a read after a `:=` in the same handler sees the written value
//!     (read-your-writes via the in-memory working state);
//!   - a handler whose committed state violates an invariant throws
//!     `InvariantViolation` and persists **nothing** (atomic revert — the gate
//!     runs before the durable write).
//!
//! It compiles a `Counter` agent in-process, then `tsc`-compiles the emitted
//! module + a driver and runs it under `node` against a fake Durable Object
//! storage. Like the tsc-verification stage it skips loudly when no TypeScript
//! toolchain is present; `BYNK_REQUIRE_TSC=1` turns the skip into a failure.

use std::fs;
use std::path::Path;
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn base_command(program: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        Command::new(program)
    }
}

fn tool_exists(name: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn discover_tsc() -> Option<(String, Vec<String>)> {
    if tool_exists("tsc") {
        return Some(("tsc".to_string(), vec![]));
    }
    if tool_exists("npx") {
        return Some((
            "npx".to_string(),
            vec![
                "--yes".to_string(),
                "-p".to_string(),
                format!("typescript@{}", bynk_emit::TYPESCRIPT_MAJOR_TESTED),
                "tsc".to_string(),
            ],
        ));
    }
    None
}

fn run(program: &str, prefix: &[String], args: &[&str], cwd: &Path) -> (bool, String) {
    let mut cmd = base_command(program);
    for p in prefix {
        cmd.arg(p);
    }
    for a in args {
        cmd.arg(a);
    }
    cmd.current_dir(cwd);
    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => return (false, format!("could not launch {program}: {e}")),
    };
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), combined)
}

const SOURCE: &str = "context shop\n\
\n\
agent Counter {\n\
\x20 key id: String\n\
\x20 store count: Cell[Int] = 0\n\
\n\
\x20 invariant nonneg: count >= 0\n\
\n\
\x20 on call set(n: Int) -> Effect[()] {\n\
\x20   count := n\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call get() -> Effect[Int] {\n\
\x20   count\n\
\x20 }\n\
\x20 on call setAndGet(n: Int) -> Effect[Int] {\n\
\x20   count := n\n\
\x20   count\n\
\x20 }\n\
}\n";

const DRIVER_TS: &str = r#"
import { Counter } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

// A fake Durable Object state: an in-memory key/value storage with the two
// methods the emitted agent uses.
function fakeState() {
  const m = new Map<string, unknown>();
  return {
    storage: {
      async get(key: string): Promise<unknown> {
        return m.get(key);
      },
      async put(key: string, value: unknown): Promise<void> {
        m.set(key, value);
      },
    },
  };
}

const c = new Counter(fakeState() as never);

// 1) A `:=` write persists, and is visible to a later handler.
await c.set(5, {});
assert((await c.get({})) === 5, "a `:=` write persists across handlers");

// 2) Read-your-writes: a read after `:=` in the same handler sees the value.
const r = await c.setAndGet(7, {});
assert(r === 7, "read-your-writes within a handler");
assert((await c.get({})) === 7, "the setAndGet write persisted");

// 3) Atomic revert: a commit that violates the invariant throws, and the
//    offending write does not persist.
let threw = false;
try {
  await c.set(-1, {});
} catch (e) {
  threw = String((e as { message?: string }).message ?? e).includes("InvariantViolation");
}
assert(threw, "an invariant-violating commit throws InvariantViolation");
assert((await c.get({})) === 7, "atomic revert: the violating write never persisted");

console.log("ALL OK");
"#;

const MAP_SOURCE: &str = "context shop\n\
\n\
agent Cart {\n\
\x20 key id: String\n\
\x20 store items: Map[String, Int]\n\
\n\
\x20 on call add(k: String, n: Int) -> Effect[()] {\n\
\x20   let _ <- items.put(k, n)\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call fetch(k: String) -> Effect[Option[Int]] {\n\
\x20   let r <- items.get(k)\n\
\x20   Effect.pure(r)\n\
\x20 }\n\
\x20 on call inc(k: String) -> Effect[()] {\n\
\x20   let _ <- items.upsert(k, 0, (x) => x + 1)\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call bumpStrict(k: String) -> Effect[()] {\n\
\x20   let _ <- items.update(k, (x) => x + 1)\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call drop(k: String) -> Effect[()] {\n\
\x20   let _ <- items.remove(k)\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call count() -> Effect[Int] {\n\
\x20   let n <- items.size()\n\
\x20   Effect.pure(n)\n\
\x20 }\n\
}\n";

const MAP_DRIVER_TS: &str = r#"
import { Cart } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

function fakeState() {
  const m = new Map<string, unknown>();
  return {
    storage: {
      async get(key: string): Promise<unknown> { return m.get(key); },
      async put(key: string, value: unknown): Promise<void> { m.set(key, value); },
    },
  };
}

const c = new Cart(fakeState() as never);

// put + get
await c.add("a", 5, {});
let r = await c.fetch("a", {});
assert(r.tag === "Some" && r.value === 5, "put then get");

// upsert on existing, then default-if-absent
await c.inc("a", {});
r = await c.fetch("a", {});
assert(r.tag === "Some" && r.value === 6, "upsert on an existing key");
await c.inc("b", {});
r = await c.fetch("b", {});
assert(r.tag === "Some" && r.value === 1, "upsert default-if-absent");

assert((await c.count({})) === 2, "size counts entries");

// remove
await c.drop("a", {});
r = await c.fetch("a", {});
assert(r.tag === "None", "remove deletes the entry");
assert((await c.count({})) === 1, "size after remove");

// update on an absent key faults — and nothing commits (atomic revert)
let threw = false;
try {
  await c.bumpStrict("missing", {});
} catch (e) {
  threw = String((e as { message?: string }).message ?? e).includes("Map.update: key absent");
}
assert(threw, "update on an absent key throws");
assert((await c.count({})) === 1, "atomic revert: the faulting update left the map unchanged");

console.log("ALL OK");
"#;

const SET_SOURCE: &str = "context shop\n\
\n\
agent Tags {\n\
\x20 key id: String\n\
\x20 store tags: Set[String]\n\
\n\
\x20 on call add(t: String) -> Effect[()] {\n\
\x20   let _ <- tags.add(t)\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call drop(t: String) -> Effect[()] {\n\
\x20   let _ <- tags.remove(t)\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call has(t: String) -> Effect[Bool] {\n\
\x20   let r <- tags.contains(t)\n\
\x20   Effect.pure(r)\n\
\x20 }\n\
\x20 on call count() -> Effect[Int] {\n\
\x20   let n <- tags.size()\n\
\x20   Effect.pure(n)\n\
\x20 }\n\
}\n";

const SET_DRIVER_TS: &str = r#"
import { Tags } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

function fakeState() {
  const m = new Map<string, unknown>();
  return {
    storage: {
      async get(key: string): Promise<unknown> { return m.get(key); },
      async put(key: string, value: unknown): Promise<void> { m.set(key, value); },
    },
  };
}

const c = new Tags(fakeState() as never);

// add + contains
assert((await c.has("x", {})) === false, "absent before add");
await c.add("x", {});
assert((await c.has("x", {})) === true, "contains after add");

// idempotent add
await c.add("x", {});
assert((await c.count({})) === 1, "add is idempotent");

await c.add("y", {});
assert((await c.count({})) === 2, "size counts members");

// remove
await c.drop("x", {});
assert((await c.has("x", {})) === false, "remove deletes the member");
assert((await c.count({})) === 1, "size after remove");

console.log("ALL OK");
"#;

// v0.98 (ADR 0125): `Cell.update(fn)` — the method-shaped read-modify-write.
// `increment` updates the cell then reads the bare name back (read-your-writes)
// and returns it; the write persists across invocations.
const CELL_UPDATE_SOURCE: &str = "context shop\n\
\n\
agent Tally {\n\
\x20 key id: String\n\
\x20 store n: Cell[Int] = 0\n\
\n\
\x20 invariant nonneg: n >= 0\n\
\n\
\x20 on call increment() -> Effect[Int] {\n\
\x20   let _ <- n.update((c) => c + 1)\n\
\x20   Effect.pure(n)\n\
\x20 }\n\
\x20 on call get() -> Effect[Int] {\n\
\x20   n\n\
\x20 }\n\
}\n";

const CELL_UPDATE_DRIVER_TS: &str = r#"
import { Tally } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

function fakeState() {
  const m = new Map<string, unknown>();
  return {
    storage: {
      async get(key: string): Promise<unknown> { return m.get(key); },
      async put(key: string, value: unknown): Promise<void> { m.set(key, value); },
    },
  };
}

const c = new Tally(fakeState() as never);

// Read-your-writes within the handler: increment returns the new value, not
// the stale prior one (the bare `n` reads the same staged slot `update` wrote).
assert((await c.increment({})) === 1, "increment returns the updated value");

// The update persists across invocations.
assert((await c.get({})) === 1, "the update persisted");
assert((await c.increment({})) === 2, "a second update reads the committed prior value");
assert((await c.get({})) === 2, "the second update persisted");

console.log("ALL OK");
"#;

// v0.116 (testing track slice 4): a `transition` (step invariant) is checked at
// the commit boundary against the pre-/post-commit state pair. `never_increases`
// forbids the balance from rising; the genesis commit has no prior state and is
// skipped, so the first `set` is always allowed.
const TRANSITION_SOURCE: &str = "context shop\n\
\n\
agent Ledger {\n\
\x20 key id: String\n\
\x20 store balance: Cell[Int] = 0\n\
\n\
\x20 transition never_increases: new.balance <= old.balance\n\
\n\
\x20 on call set(n: Int) -> Effect[()] {\n\
\x20   balance := n\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call get() -> Effect[Int] {\n\
\x20   balance\n\
\x20 }\n\
}\n";

const TRANSITION_DRIVER_TS: &str = r#"
import { Ledger } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

function fakeState() {
  const m = new Map<string, unknown>();
  return {
    storage: {
      async get(key: string): Promise<unknown> { return m.get(key); },
      async put(key: string, value: unknown): Promise<void> { m.set(key, value); },
    },
  };
}

const c = new Ledger(fakeState() as never);

// 1) Genesis: the first commit has no prior state, so the transition is skipped —
//    set(5) is allowed even though 5 > 0 (the default), which `never_increases`
//    would otherwise forbid.
await c.set(5, {});
assert((await c.get({})) === 5, "the genesis commit skips the transition check");

// 2) A legal step (3 <= 5) commits.
await c.set(3, {});
assert((await c.get({})) === 3, "a legal step commits");

// 3) An illegal step (10 <= 3 is false) throws InvariantViolation and persists
//    nothing (atomic revert — the gate runs before the durable write).
let threw = false;
try {
  await c.set(10, {});
} catch (e) {
  threw = String((e as { message?: string }).message ?? e).includes("InvariantViolation");
}
assert(threw, "an illegal transition throws InvariantViolation");
assert((await c.get({})) === 3, "atomic revert: the illegal step never persisted");

console.log("ALL OK");
"#;

const CACHE_SOURCE: &str = "context shop\n\
\n\
capability Clock {\n\
\x20 fn now() -> Effect[Int]\n\
}\n\
\n\
provides Clock = SystemClock {\n\
\x20 fn now() -> Effect[Int] {\n\
\x20   0\n\
\x20 }\n\
}\n\
\n\
agent Sessions {\n\
\x20 key id: String\n\
\x20 store live: Cache[String, Int] @ttl(1.minutes)\n\
\n\
\x20 on call put(k: String, n: Int) -> Effect[()] given Clock {\n\
\x20   let _ <- live.put(k, n)\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call fetch(k: String) -> Effect[Option[Int]] given Clock {\n\
\x20   let r <- live.get(k)\n\
\x20   Effect.pure(r)\n\
\x20 }\n\
\x20 on call has(k: String) -> Effect[Bool] given Clock {\n\
\x20   let b <- live.contains(k)\n\
\x20   Effect.pure(b)\n\
\x20 }\n\
\x20 on call count() -> Effect[Int] given Clock {\n\
\x20   let n <- live.size()\n\
\x20   Effect.pure(n)\n\
\x20 }\n\
}\n";

const CACHE_DRIVER_TS: &str = r#"
import { Sessions } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

function fakeState() {
  const m = new Map<string, unknown>();
  return {
    storage: {
      async get(key: string): Promise<unknown> { return m.get(key); },
      async put(key: string, value: unknown): Promise<void> { m.set(key, value); },
    },
  };
}

// A controllable mock clock — the testability the `given Clock` design buys
// (ADR 0113 D4). `nowMs` is advanced by the driver to drive TTL expiry.
let nowMs = 1_000_000;
const clock = { now: async (): Promise<number> => nowMs };
const deps = { Clock: clock };

const c = new Sessions(fakeState() as never);

// put + get within the TTL window
await c.put("a", 5, deps);
let r = await c.fetch("a", deps);
assert(r.tag === "Some" && r.value === 5, "live entry reads back");
assert((await c.has("a", deps)) === true, "contains is true while live");
assert((await c.count(deps)) === 1, "size counts the live entry");

// advance past the 1-minute TTL — the entry expires (lazy, check-on-read)
nowMs += 60_001;
r = await c.fetch("a", deps);
assert(r.tag === "None", "an entry past its TTL reads as None");
assert((await c.has("a", deps)) === false, "contains is false once expired");
assert((await c.count(deps)) === 0, "size drops the expired entry");

// a fresh put resets the lifetime
await c.put("a", 9, deps);
r = await c.fetch("a", deps);
assert(r.tag === "Some" && r.value === 9, "put resets the entry's TTL");

console.log("ALL OK");
"#;

// v0.94 (ADR 0116/0120): joins & grouping in the combiner form. A storage
// `joinOn`/`leftJoin`/`groupBy` over two store maps (the lazy `Query` lowering),
// plus an in-memory `List` join (the eager lowering), projecting each result
// through `into` — there is no pair value.
const JOIN_SOURCE: &str = "context shop\n\
\n\
type Order = {\n\
\x20 id: String,\n\
\x20 customer: String,\n\
}\n\
\n\
type Line = {\n\
\x20 orderId: String,\n\
\x20 qty: Int,\n\
}\n\
\n\
type Joined = {\n\
\x20 customer: String,\n\
\x20 qty: Int,\n\
}\n\
\n\
type Tot = {\n\
\x20 orderId: String,\n\
\x20 total: Int,\n\
}\n\
\n\
agent Sales {\n\
\x20 key k: String\n\
\x20 store orders: Map[String, Order]\n\
\x20 store lines: Map[String, Line]\n\
\n\
\x20 on call addOrder(id: String, c: String) -> Effect[()] {\n\
\x20   let _ <- orders.put(id, Order { id: id, customer: c })\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call addLine(lid: String, oid: String, q: Int) -> Effect[()] {\n\
\x20   let _ <- lines.put(lid, Line { orderId: oid, qty: q })\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call innerCount() -> Effect[Int] {\n\
\x20   lines.joinOn(orders, (l) => l.orderId, (o) => o.id, (l, o) => Joined { customer: o.customer, qty: l.qty }).count()\n\
\x20 }\n\
\x20 on call innerQty() -> Effect[Int] {\n\
\x20   lines.joinOn(orders, (l) => l.orderId, (o) => o.id, (l, o) => Joined { customer: o.customer, qty: l.qty }).sum((j) => j.qty)\n\
\x20 }\n\
\x20 on call leftCount() -> Effect[Int] {\n\
\x20   lines.leftJoin(orders, (l) => l.orderId, (o) => o.id, (l, mo) => Joined { customer: \"x\", qty: l.qty }).count()\n\
\x20 }\n\
\x20 on call groupCount() -> Effect[Int] {\n\
\x20   lines.groupBy((l) => l.orderId, (oid, rows) => Tot { orderId: oid, total: rows.sum((r) => r.qty) }).count()\n\
\x20 }\n\
\x20 on call listJoin(os: List[Order], ls: List[Line]) -> Effect[Int] {\n\
\x20   Effect.pure(ls.joinOn(os, (l) => l.orderId, (o) => o.id, (l, o) => Joined { customer: o.customer, qty: l.qty }).length())\n\
\x20 }\n\
}\n";

const JOIN_DRIVER_TS: &str = r#"
import { Sales } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

function fakeState() {
  const m = new Map<string, unknown>();
  return {
    storage: {
      async get(key: string): Promise<unknown> { return m.get(key); },
      async put(key: string, value: unknown): Promise<void> { m.set(key, value); },
    },
  };
}

const c = new Sales(fakeState() as never);

// Two orders; four lines, one (l4) referencing a non-existent order o3.
await c.addOrder("o1", "alice", {});
await c.addOrder("o2", "bob", {});
await c.addLine("l1", "o1", 5, {});
await c.addLine("l2", "o1", 3, {});
await c.addLine("l3", "o2", 7, {});
await c.addLine("l4", "o3", 9, {});

// Inner equi-join: l1/l2/l3 match; l4 (o3) has no order → dropped.
assert((await c.innerCount({})) === 3, "joinOn keeps only matched rows");
assert((await c.innerQty({})) === 15, "joinOn projects through into (5+3+7)");

// Left join: every line survives, l4 with the unmatched (None) branch → 4.
assert((await c.leftCount({})) === 4, "leftJoin keeps unmatched left rows");

// groupBy: three distinct orderIds (o1, o2, o3) → three groups.
assert((await c.groupCount({})) === 3, "groupBy partitions by key");

// In-memory List join (eager lowering): same matching as the storage join.
const os = [{ id: "o1", customer: "alice" }, { id: "o2", customer: "bob" }];
const ls = [
  { orderId: "o1", qty: 5 }, { orderId: "o1", qty: 3 },
  { orderId: "o2", qty: 7 }, { orderId: "o3", qty: 9 },
];
assert((await c.listJoin(os, ls, {})) === 3, "in-memory joinOn matches like the storage join");

console.log("ALL OK");
"#;

// v0.93 (ADR 0118): a `store Map @indexed(by: orderId)` — the secondary index is
// maintained inside the commit (re-index on last-write-wins / update / remove)
// and an equality `filter` on the indexed field routes to a posting-list lookup.
const INDEX_SOURCE: &str = "context shop\n\
\n\
type Reservation = {\n\
\x20 id: String,\n\
\x20 orderId: String,\n\
\x20 qty: Int,\n\
}\n\
\n\
agent Inventory {\n\
\x20 key sku: String\n\
\x20 store reservations: Map[String, Reservation] @indexed(by: orderId)\n\
\n\
\x20 on call reserve(rid: String, oid: String, n: Int) -> Effect[()] {\n\
\x20   let _ <- reservations.put(rid, Reservation { id: rid, orderId: oid, qty: n })\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call retag(rid: String, oid: String) -> Effect[()] {\n\
\x20   let _ <- reservations.update(rid, (r) => Reservation { ...r, orderId: oid })\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call setQty(rid: String, n: Int) -> Effect[()] {\n\
\x20   let _ <- reservations.update(rid, (r) => Reservation { ...r, qty: n })\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call drop(rid: String) -> Effect[()] {\n\
\x20   let _ <- reservations.remove(rid)\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call countForOrder(oid: String) -> Effect[Int] {\n\
\x20   reservations.filter((r) => r.orderId == oid).count()\n\
\x20 }\n\
\x20 on call qtyForOrder(oid: String) -> Effect[Int] {\n\
\x20   reservations.filter((r) => r.orderId == oid).sum((r) => r.qty)\n\
\x20 }\n\
}\n";

const INDEX_DRIVER_TS: &str = r#"
import { Inventory } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

function fakeState() {
  const m = new Map<string, unknown>();
  return {
    storage: {
      async get(key: string): Promise<unknown> { return m.get(key); },
      async put(key: string, value: unknown): Promise<void> { m.set(key, value); },
    },
  };
}

const c = new Inventory(fakeState() as never);

// Two reservations on o1, one on o2.
await c.reserve("r1", "o1", 5, {});
await c.reserve("r2", "o1", 3, {});
await c.reserve("r3", "o2", 7, {});
assert((await c.countForOrder("o1", {})) === 2, "indexed filter returns both o1 rows");
assert((await c.qtyForOrder("o1", {})) === 8, "indexed filter sums o1 (5+3)");
assert((await c.countForOrder("o2", {})) === 1, "indexed filter returns the o2 row");
assert((await c.countForOrder("o3", {})) === 0, "an unseen key yields no rows");

// Re-tag r2 from o1 to o2 (update the indexed field) — postings must move.
await c.retag("r2", "o2", {});
assert((await c.countForOrder("o1", {})) === 1, "re-index drops the old posting");
assert((await c.countForOrder("o2", {})) === 2, "re-index adds the new posting");
assert((await c.qtyForOrder("o2", {})) === 10, "re-index keeps sums correct (7+3)");

// Last-write-wins: re-put r1 under a different order.
await c.reserve("r1", "o2", 5, {});
assert((await c.countForOrder("o1", {})) === 0, "last-write-wins drops the stale posting");
assert((await c.countForOrder("o2", {})) === 3, "last-write-wins adds the new posting");

// Remove drops the posting.
await c.drop("r3", {});
assert((await c.countForOrder("o2", {})) === 2, "remove drops the posting");

// A non-indexed update keeps the index but changes the value.
await c.setQty("r1", 100, {});
assert((await c.countForOrder("o2", {})) === 2, "a non-indexed update leaves postings intact");
assert((await c.qtyForOrder("o2", {})) === 103, "the updated value flows through (3+100)");

console.log("ALL OK");
"#;

const LOG_SOURCE: &str = "context shop\n\
\n\
capability Clock {\n\
\x20 fn now() -> Effect[Instant]\n\
}\n\
\n\
provides Clock = SystemClock {\n\
\x20 fn now() -> Effect[Instant] {\n\
\x20   Instant.fromEpochMillis(0)\n\
\x20 }\n\
}\n\
\n\
agent Audit {\n\
\x20 key id: String\n\
\x20 store events: Log[Int] @retain(1.minutes)\n\
\n\
\x20 on call write(n: Int) -> Effect[()] given Clock {\n\
\x20   let _ <- events.append(n)\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call dump() -> Effect[List[Int]] {\n\
\x20   events.collect()\n\
\x20 }\n\
\x20 on call countSince(t: Instant) -> Effect[Int] {\n\
\x20   events.since(t).count()\n\
\x20 }\n\
\x20 on call lastN(n: Int) -> Effect[List[Int]] {\n\
\x20   events.recent(n).collect()\n\
\x20 }\n\
}\n";

const LOG_DRIVER_TS: &str = r#"
import { Audit } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}
function eq<T>(a: T, b: T, msg: string): void {
  assert(JSON.stringify(a) === JSON.stringify(b), `${msg} (got ${JSON.stringify(a)})`);
}

function fakeState() {
  const m = new Map<string, unknown>();
  return {
    storage: {
      async get(key: string): Promise<unknown> { return m.get(key); },
      async put(key: string, value: unknown): Promise<void> { m.set(key, value); },
    },
  };
}

// A controllable mock clock (the testability `given Clock` buys, ADR 0121 D2).
let nowMs = 1_000;
const clock = { now: async (): Promise<number> => nowMs };
const deps = { Clock: clock };

const c = new Audit(fakeState() as never);

// append stamps the clock; advance it between writes
await c.write(1, deps);                 // t=1000
nowMs = 2_000; await c.write(2, deps);  // t=2000
nowMs = 3_000; await c.write(3, deps);  // t=3000

// collect (clock-free read) yields the values in append order
eq(await c.dump({}), [1, 2, 3], "collect is append-ordered");

// since(Instant) filters by timestamp — a clock-free read (explicit bound)
assert((await c.countSince(2_000, {})) === 2, "since(2000) keeps t>=2000");
assert((await c.countSince(3_001, {})) === 0, "since after the last entry is empty");

// recent(n) is the last n, newest first
eq(await c.lastN(2, {}), [3, 2], "recent(2) is newest-first");

// @retain prunes on append: t=70000 drops everything older than 70000-60000
nowMs = 70_000; await c.write(4, deps);
eq(await c.dump({}), [4], "retention prunes entries past the window on append");

console.log("ALL OK");
"#;

/// #1660 (runtime-semantics track §3.2/§3.5): a commit refused by an invariant
/// must leave **every** store untouched, including a collection written *in
/// place*.
///
/// The `Cell` case in `DRIVER_TS` (step 3) never exposed the gap. A handler starts
/// from a *shallow* copy of the loaded state, and `:=` replaces a top-level field
/// of that copy, so it never reaches storage. A store-`Map` `put` instead writes
/// *into* the nested record (`__state.items[k] = v`). When the storage handed back
/// its own reference, as the runtime's `InMemoryStorage` did before #1660, that
/// write landed in storage before the commit decision, and a refused commit had
/// already happened.
///
/// This drives the bundle's own `__makeCart` factory, and with it the runtime's
/// `StateRegistry`/`InMemoryStorage` rather than a fake, so it pins the storage
/// `bynkc test` and bundle deployments actually use.
const REFUSED_LEAK_SOURCE: &str = "context shop\n\
\n\
agent Cart {\n\
\x20 key id: String\n\
\x20 store items: Map[String, Int]\n\
\x20 store total: Cell[Int] = 0\n\
\n\
\x20 invariant small: total <= 1\n\
\n\
\x20 on call add(k: String) -> Effect[()] {\n\
\x20   do items.put(k, 1)\n\
\x20   do total.update((t) => t + 1)\n\
\x20   ()\n\
\x20 }\n\
\x20 on call count() -> Effect[Int] {\n\
\x20   let n <- items.size()\n\
\x20   n\n\
\x20 }\n\
}\n";

const REFUSED_LEAK_DRIVER_TS: &str = r#"
import { __makeCart } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

// The emitted agent logs the violation it refuses; keep the run's output clean.
console.error = () => {};

const cart = __makeCart("k");
await cart.add("a", {});
assert((await cart.count({})) === 1, "the first add commits");

let threw = false;
try {
  await cart.add("b", {});
} catch (e) {
  threw = String((e as { message?: string }).message ?? e).includes("InvariantViolation");
}
assert(threw, "the second add is refused by `invariant small`");
assert(
  (await cart.count({})) === 1,
  "a refused commit's in-place store-Map write must not persist",
);

console.log("ALL OK");
"#;

const TSCONFIG_JSON: &str = r#"{
  "compilerOptions": {
    "module": "Node16",
    "moduleResolution": "node16",
    "target": "ES2022",
    "strict": true,
    "skipLibCheck": true,
    "outDir": "js",
    "rootDir": ".",
    "lib": ["ES2022", "DOM"]
  },
  "include": ["**/*.ts"],
  "exclude": ["js"]
}
"#;

/// Compile `source` (a one-file `context shop` project, bundle target), write it
/// alongside `driver`, then `tsc`-compile and run the driver under node, asserting
/// it prints `ALL OK`. Skips loudly without a TS toolchain.
fn verify(tag: &str, source: &str, driver: &str) {
    verify_with(tag, source, driver, bynkc::BuildTarget::Bundle);
}

/// [`verify`] for a chosen build target. #1691: a held `Map[K, Connection]` is
/// lowered by its own branch only on Workers (the record stores connection ids,
/// re-resolved through the Durable Object's hibernation API), so its driver
/// compiles for Workers and imports `workers/shop/handlers.js`.
fn verify_with(tag: &str, source: &str, driver: &str, target: bynkc::BuildTarget) {
    verify_built(tag, source, driver, target, bynkc::SchemaLock::Off);
}

/// [`verify`] for a build that reconciles against a schema lock — `existing`
/// is the committed lock's content, `None` for a fresh project — so agents
/// emit the state-migrations S1 version logic (#2211 Decision A).
fn verify_with_lock(tag: &str, source: &str, driver: &str, existing: Option<&str>) {
    verify_built(
        tag,
        source,
        driver,
        bynkc::BuildTarget::Bundle,
        bynkc::SchemaLock::On {
            existing: existing.map(str::to_string),
        },
    );
}

fn verify_built(
    tag: &str,
    source: &str,
    driver: &str,
    target: bynkc::BuildTarget,
    schema_lock: bynkc::SchemaLock,
) {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!("\n!!! STORE-BEHAVIOUR VERIFICATION SKIPPED !!!\nno `tsc`/`npx` on PATH.\n");
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! STORE-BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let tmp =
        std::env::temp_dir().join(format!("bynk-store-behaviour-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let src = tmp.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("shop.bynk"), source).unwrap();
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_single(src.clone())
            .target(target)
            .schema_registry(schema_lock),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .expect("the store agent must compile");

    for (path, doc) in &out.artefacts.docs {
        if path.to_string_lossy() == "tsconfig.json" {
            continue;
        }
        let target_path = tmp.join(path);
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&target_path, doc.text()).unwrap();
    }
    fs::write(tmp.join("driver.ts"), driver).unwrap();
    fs::write(tmp.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    fs::write(tmp.join("package.json"), "{ \"type\": \"module\" }").unwrap();

    let (program, prefix) = &runner;
    let (ok, out_text) = run(program, prefix, &["-p", "tsconfig.json"], &tmp);
    assert!(
        ok,
        "tsc failed on the store-agent driver ({tag}):\n{out_text}"
    );

    let (ok, out_text) = run("node", &[], &["js/driver.js"], &tmp);
    let _ = fs::remove_dir_all(&tmp);
    assert!(
        ok && out_text.contains("ALL OK"),
        "store-agent behaviour driver ({tag}) did not pass:\n{out_text}"
    );
}

/// #1691: a held connection map (`Map[String, Connection[F]]`), lowered by
/// `lower.rs`'s held-map branch on Workers: `put` stores the connection's id with
/// an own-property write, `contains`/`get` test with `Object.hasOwn`, `get`
/// re-resolves the live socket, `remove` closes it and deletes the entry.
const HELD_MAP_SOURCE: &str = "context shop\n\
\n\
type ServerFrame = { text: String }\n\
\n\
agent Room {\n\
\x20 key id: String\n\
\x20 store conns: Map[String, Connection[ServerFrame]]\n\
\n\
\x20 on call join(u: String, conn: Connection[ServerFrame]) -> Effect[()] {\n\
\x20   let _ <- conns.put(u, conn)\n\
\x20   ()\n\
\x20 }\n\
\x20 on call has(u: String) -> Effect[Bool] {\n\
\x20   let b <- conns.contains(u)\n\
\x20   Effect.pure(b)\n\
\x20 }\n\
\x20 on call present(u: String) -> Effect[Bool] {\n\
\x20   let c <- conns.get(u)\n\
\x20   match c {\n\
\x20     Some(_) => true\n\
\x20     None => false\n\
\x20   }\n\
\x20 }\n\
\x20 on call leave(u: String) -> Effect[()] {\n\
\x20   let _ <- conns.remove(u)\n\
\x20   ()\n\
\x20 }\n\
\x20 on call count() -> Effect[Int] {\n\
\x20   let n <- conns.size()\n\
\x20   Effect.pure(n)\n\
\x20 }\n\
\x20 on call ping(u: String) -> Effect[()] {\n\
\x20   let c <- conns.get(u)\n\
\x20   match c {\n\
\x20     Some(conn) => {\n\
\x20       let _ <- conn.send(ServerFrame { text: u })\n\
\x20       conns.put(u, conn)\n\
\x20     },\n\
\x20     None => Effect.pure(())\n\
\x20   }\n\
\x20 }\n\
\x20 on call broadcast(text: String) -> Effect[()] {\n\
\x20   conns.parTraverse((c: Connection[ServerFrame]) => c.send(ServerFrame { text: text }))\n\
\x20 }\n\
}\n";

/// Drives the held map with keys that name `Object.prototype` members
/// (`__proto__`, `constructor`, `toString`) beside an ordinary one, through a
/// fake Durable Object state: storage, plus the hibernatable-WebSocket surface
/// (`acceptWebSocket`/`getWebSockets`) that `resolveConnection` re-presents a
/// socket through. Connections come from the runtime's own
/// `acceptHibernatableConnection`, so their ids are real connIds.
const HELD_MAP_DRIVER_TS: &str = r#"
import { Room } from "./workers/shop/handlers.js";
import { acceptHibernatableConnection } from "./__bynk_runtime/connection.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

interface FakeWs {
  closed: boolean;
  sent: string[];
  accept(): void;
  send(data: string): void;
  close(): void;
  serializeAttachment(value: unknown): void;
  deserializeAttachment(): unknown;
}

function fakeWs(): FakeWs {
  let attachment: unknown;
  const ws: FakeWs = {
    closed: false,
    sent: [],
    accept(): void {},
    send(data: string): void { ws.sent.push(data); },
    close(): void { ws.closed = true; },
    serializeAttachment(value: unknown): void { attachment = value; },
    deserializeAttachment(): unknown { return attachment; },
  };
  return ws;
}

// Storage round-trips through structuredClone, like a real DO's.
function fakeState() {
  const m = new Map<string, unknown>();
  const sockets = new Map<string, FakeWs>();
  return {
    storage: {
      async get(key: string): Promise<unknown> { return structuredClone(m.get(key)); },
      async put(key: string, value: unknown): Promise<void> { m.set(key, structuredClone(value)); },
    },
    acceptWebSocket(ws: FakeWs, tags?: string[]): void {
      for (const t of tags ?? []) sockets.set(t, ws);
    },
    getWebSockets(tag?: string): FakeWs[] {
      // The lowering only ever hands `resolveConnection` a stored connId; an
      // inherited value (`Object.prototype`, a builtin method) reaching here
      // means a lookup read through the prototype chain.
      if (typeof tag !== "string") throw new Error(`getWebSockets: non-string tag ${typeof tag}`);
      const ws = sockets.get(tag);
      return ws === undefined || ws.closed ? [] : [ws];
    },
  };
}

const state = fakeState();
const room = new Room(state as never);
const keys = ["__proto__", "constructor", "toString", "alice"];
// A Map, not an object literal: `obj["__proto__"] = ws` would hit the
// prototype setter rather than store an own entry.
const sockets = new Map<string, FakeWs>();
const sock = (k: string): FakeWs => sockets.get(k)!;
const texts = (k: string): string[] => sock(k).sent.map((d) => JSON.parse(d).text);

for (const k of keys) {
  const ws = fakeWs();
  sockets.set(k, ws);
  await room.join(k, acceptHibernatableConnection(state as never, ws as never), {});
}

assert((await room.count({})) === 4, "size counts all four entries, prototype names included");
for (const k of keys) {
  assert((await room.has(k, {})) === true, `contains ${k}`);
  assert((await room.present(k, {})) === true, `get ${k} resolves its connection`);
}
for (const k of ["hasOwnProperty", "valueOf", "isPrototypeOf"]) {
  assert((await room.has(k, {})) === false, `contains ${k}: never stored, not inherited`);
  assert((await room.present(k, {})) === false, `get ${k}: never stored, not inherited`);
}

// get resolves each key to its own socket, and put-back keeps the entry
for (const k of keys) await room.ping(k, {});
for (const k of keys) {
  assert(JSON.stringify(texts(k)) === JSON.stringify([k]), `get ${k} sends on ${k}'s socket only`);
}
assert((await room.count({})) === 4, "ping's put-back leaves all four entries");

// remove of a never-stored inherited name is a no-op
await room.leave("valueOf", {});
assert((await room.count({})) === 4, "remove valueOf: never stored, nothing removed");

// remove closes that entry's socket and deletes only it
await room.leave("__proto__", {});
assert(sock("__proto__").closed, "remove closes the __proto__ connection");
assert((await room.has("__proto__", {})) === false, "remove deletes the __proto__ entry");
assert((await room.count({})) === 3, "size after removing __proto__");
await room.leave("constructor", {});
assert(sock("constructor").closed, "remove closes the constructor connection");
assert(!sock("toString").closed && !sock("alice").closed, "remove touches only its own entry");
assert((await room.present("toString", {})) === true, "toString still resolves");
assert((await room.count({})) === 2, "size after removing constructor");

// a non-entry op lifts the map over its own values: only the live entries hear it
await room.broadcast("hi", {});
for (const k of ["toString", "alice"]) assert(texts(k).at(-1) === "hi", `broadcast reaches ${k}`);
for (const k of ["__proto__", "constructor"]) assert(!texts(k).includes("hi"), `broadcast skips removed ${k}`);

console.log("ALL OK");
"#;

#[test]
fn store_cell_agent_runtime_semantics() {
    verify("cell", SOURCE, DRIVER_TS);
}

#[test]
fn store_cell_update_runtime_semantics() {
    verify("cell_update", CELL_UPDATE_SOURCE, CELL_UPDATE_DRIVER_TS);
}

#[test]
fn store_map_agent_runtime_semantics() {
    verify("map", MAP_SOURCE, MAP_DRIVER_TS);
}

#[test]
fn store_set_agent_runtime_semantics() {
    verify("set", SET_SOURCE, SET_DRIVER_TS);
}

#[test]
fn store_indexed_map_runtime_semantics() {
    verify("index", INDEX_SOURCE, INDEX_DRIVER_TS);
}

#[test]
fn query_join_and_group_runtime_semantics() {
    verify("join", JOIN_SOURCE, JOIN_DRIVER_TS);
}

#[test]
fn store_log_agent_runtime_semantics() {
    verify("log", LOG_SOURCE, LOG_DRIVER_TS);
}

#[test]
fn store_cache_agent_runtime_semantics() {
    verify("cache", CACHE_SOURCE, CACHE_DRIVER_TS);
}

#[test]
fn transition_step_invariant_runtime_semantics() {
    verify("transition", TRANSITION_SOURCE, TRANSITION_DRIVER_TS);
}

// v0.96 (ADR 0124): the rehydration validation gate. A loaded refined field that
// fails its predicate — or a structurally-corrupt one — faults with a
// `RehydrationViolation` before any handler reads it (Q6); a `store` field absent
// from a record written before it existed takes its default (D4 additive
// evolution), without faulting.
const REHYDRATE_SOURCE: &str = "context shop\n\
\n\
type Pos = Int where Positive\n\
\n\
agent Gauge {\n\
\x20 key id: String\n\
\n\
\x20 store level: Cell[Pos] = 1\n\
\x20 store note:  Cell[String]\n\
\n\
\x20 on call read() -> Effect[Int] {\n\
\x20   level\n\
\x20 }\n\
}\n";

const REHYDRATE_DRIVER_TS: &str = r#"
import { Gauge } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

function fakeState() {
  const m = new Map<string, unknown>();
  return {
    storage: {
      async get(key: string): Promise<unknown> { return m.get(key); },
      async put(key: string, value: unknown): Promise<void> { m.set(key, value); },
    },
  };
}

// 1) A refined field that violates its predicate on load faults — and as a
//    `RehydrationViolation`, naming the agent, never the value.
const st1 = fakeState();
await st1.storage.put("state", { level: -5, note: "x" });
const g1 = new Gauge(st1 as never);
let threw1 = false;
try {
  await g1.read({});
} catch (e) {
  const rv = (e as { rehydrationViolation?: { kind: string; agent: string } }).rehydrationViolation;
  threw1 = rv?.kind === "RehydrationViolation" && rv?.agent === "Gauge";
}
assert(threw1, "a refined field failing on load throws RehydrationViolation");

// 2) A structurally-corrupt field (wrong type) faults too.
const st2 = fakeState();
await st2.storage.put("state", { level: "oops", note: "x" });
const g2 = new Gauge(st2 as never);
let threw2 = false;
try {
  await g2.read({});
} catch (e) {
  threw2 = (e as { rehydrationViolation?: { kind: string } }).rehydrationViolation?.kind === "RehydrationViolation";
}
assert(threw2, "a structurally-corrupt field throws RehydrationViolation");

// 3) Additive evolution: `note` is absent from a record written before the field
//    existed, so it takes its zero — no fault — and the valid `level` is returned.
const st3 = fakeState();
await st3.storage.put("state", { level: 2 });
const g3 = new Gauge(st3 as never);
assert((await g3.read({})) === 2, "an absent (additive) field defaults rather than faulting");

// 4) A fully-valid stored record rehydrates cleanly.
const st4 = fakeState();
await st4.storage.put("state", { level: 3, note: "ok" });
const g4 = new Gauge(st4 as never);
assert((await g4.read({})) === 3, "valid stored state rehydrates");

console.log("ALL OK");
"#;

#[test]
fn store_rehydration_gate_runtime_semantics() {
    verify("rehydration", REHYDRATE_SOURCE, REHYDRATE_DRIVER_TS);
}

// #2607 (Decision F): rehydration decodes a stored record through the same
// codec a wire body passes, so a record held in a `Cell` that has since gained
// a defaulted field loads an instance stored before the field existed, and the
// field reads as its default.
const REHYDRATE_DEFAULT_SOURCE: &str = "context shop\n\
\n\
type Prefs = { theme: String, size: Int = 12 }\n\
\n\
agent Profile {\n\
\x20 key id: String\n\
\n\
\x20 store prefs: Cell[Prefs] = Prefs { theme: \"light\" }\n\
\n\
\x20 on call size() -> Effect[Int] {\n\
\x20   prefs.size\n\
\x20 }\n\
}\n";

const REHYDRATE_DEFAULT_DRIVER_TS: &str = r#"
import { Profile } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

function fakeState() {
  const m = new Map<string, unknown>();
  return {
    storage: {
      async get(key: string): Promise<unknown> { return m.get(key); },
      async put(key: string, value: unknown): Promise<void> { m.set(key, value); },
    },
  };
}

// 1) Stored before `size` existed: it reads as its default.
const st1 = fakeState();
await st1.storage.put("state", { prefs: { theme: "dark" } });
assert((await new Profile(st1 as never).size({})) === 12, "an absent defaulted field reads as its default");

// 2) A stored value is kept.
const st2 = fakeState();
await st2.storage.put("state", { prefs: { theme: "dark", size: 9 } });
assert((await new Profile(st2 as never).size({})) === 9, "a stored value is kept");

// 3) A fresh instance's initialiser takes the default too.
assert((await new Profile(fakeState() as never).size({})) === 12, "a fresh instance takes the default");

console.log("ALL OK");
"#;

#[test]
fn rehydration_fills_a_defaulted_record_field() {
    verify(
        "rehydration_default",
        REHYDRATE_DEFAULT_SOURCE,
        REHYDRATE_DEFAULT_DRIVER_TS,
    );
}

#[test]
fn refused_commit_does_not_leak_an_in_place_store_write() {
    verify("refused_leak", REFUSED_LEAK_SOURCE, REFUSED_LEAK_DRIVER_TS);
}

// #1649 (ADR agent-state-wire-shape, D2/D3): stored state is in the wire shape,
// and the load-time decode runs per field, only for fields present in the stored
// record. Every fixture commits through `__encode<Agent>State`, which writes
// every declared field, so no fixture ever loads a record with a field missing,
// or with a key the definition no longer declares. This test builds both by
// editing a committed record:
//
//   - a declared field absent from the stored record (an additive deploy) keeps
//     its zero and is not decoded. Decoding the in-memory zero as if it were wire
//     data would fault with `RehydrationViolation`;
//   - a stored key the definition does not declare (a renamed or removed field)
//     survives the load and the next commit, for #539's migrations to find.
//
// The fake storage clones on `get` and `put`, as workerd's storage does and as
// the bundle target's in-memory stand-in does since #1660. The decode writes
// through the stored record's nested objects (ADR consequences), so a backend
// that handed back the stored object itself would double-decode.
const STATE_SHAPE_SOURCE: &str = "context shop\n\
\n\
type Light = enum { Red, Green }\n\
\n\
agent Lamp {\n\
\x20 key id: String\n\
\n\
\x20 store light: Cell[Light] = Red\n\
\x20 store reading: Cell[Option[Int]]\n\
\x20 store board: Map[String, Light]\n\
\n\
\x20 on call setAll() -> Effect[()] {\n\
\x20   light := Green\n\
\x20   reading := Some(3)\n\
\x20   let _ <- board.put(\"a\", Green)\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call touch() -> Effect[()] {\n\
\x20   light := Green\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call isGreen() -> Effect[Bool] {\n\
\x20   light == Green\n\
\x20 }\n\
\x20 on call lastReading() -> Effect[Int] {\n\
\x20   match reading {\n\
\x20     Some(n) => n\n\
\x20     None => 0\n\
\x20   }\n\
\x20 }\n\
\x20 on call boardSize() -> Effect[Int] {\n\
\x20   let n <- board.size()\n\
\x20   Effect.pure(n)\n\
\x20 }\n\
}\n";

const STATE_SHAPE_DRIVER_TS: &str = r#"
import { Lamp } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

function fakeState() {
  const m = new Map<string, unknown>();
  return {
    storage: {
      async get(key: string): Promise<unknown> { return structuredClone(m.get(key)); },
      async put(key: string, value: unknown): Promise<void> { m.set(key, structuredClone(value)); },
    },
  };
}

// Commit a full record, then rewrite it as an older/renamed deployment would
// have left it: `reading` and `board` absent, an undeclared `legacy` present.
// The committed shape is reused, not spelled out, so this test does not pin the
// wire encoding.
const st = fakeState();
await new Lamp(st as never).setAll({});
const raw = (await st.storage.get("state")) as Record<string, unknown>;
assert("reading" in raw && "board" in raw, "a commit stores every declared field");
delete raw.reading;
delete raw.board;
raw.legacy = 42;
await st.storage.put("state", raw);

// 1) The present field decodes; the absent ones keep their zero without
//    faulting.
const lamp = new Lamp(st as never);
assert(await lamp.isGreen({}), "a present field decodes from the wire shape");
assert((await lamp.lastReading({})) === 0, "an absent Option field keeps its zero (None)");
assert((await lamp.boardSize({})) === 0, "an absent Map field keeps its zero (empty)");

// 2) A commit writes the absent fields back and carries the undeclared key.
await lamp.touch({});
const after = (await st.storage.get("state")) as Record<string, unknown>;
assert(after.legacy === 42, "an undeclared stored key survives a load and a commit");
assert("reading" in after && "board" in after, "the commit writes the defaulted fields");

// 3) The rewritten record loads again.
assert(await lamp.isGreen({}), "the record reloads after the commit");
assert((await lamp.lastReading({})) === 0, "the defaulted field reloads as its zero");

console.log("ALL OK");
"#;

#[test]
fn agent_state_keeps_unknown_keys_and_defaults_missing_fields() {
    verify("state_shape", STATE_SHAPE_SOURCE, STATE_SHAPE_DRIVER_TS);
}

// #1685: `update` on a key the collection does not hold faults, including a key
// named after an `Object.prototype` member. Before #1685 the `in` check read
// `constructor` as present, so `update("constructor", f)` ran `f` over the
// inherited `Object` function and stored the result. A suite cannot assert a
// fault, so the driver catches it here. All three `update` lowerings (store
// `Map`, `Cache`, `@indexed` map) are covered, since each writes with a plain
// set that is sound only behind its own-key check.
const UPDATE_ABSENT_SOURCE: &str = "context shop\n\
\n\
capability Clock {\n\
\x20 fn now() -> Effect[Int]\n\
}\n\
\n\
provides Clock = SystemClock {\n\
\x20 fn now() -> Effect[Int] {\n\
\x20   0\n\
\x20 }\n\
}\n\
\n\
type Entry = { id: String, tag: String }\n\
\n\
agent Keys {\n\
\x20 key id: String\n\
\x20 store items: Map[String, Int]\n\
\x20 store live: Cache[String, Int] @ttl(1.minutes)\n\
\x20 store entries: Map[String, Entry] @indexed(by: tag)\n\
\n\
\x20 on call bumpItem(k: String) -> Effect[()] {\n\
\x20   let _ <- items.update(k, (n) => n + 1)\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call bumpLive(k: String) -> Effect[()] given Clock {\n\
\x20   let _ <- live.update(k, (n) => n + 1)\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call retag(k: String, t: String) -> Effect[()] {\n\
\x20   let _ <- entries.update(k, (e) => Entry { ...e, tag: t })\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call itemCount() -> Effect[Int] {\n\
\x20   let n <- items.size()\n\
\x20   Effect.pure(n)\n\
\x20 }\n\
}\n";

const UPDATE_ABSENT_DRIVER_TS: &str = r#"
import { Keys } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

function fakeState() {
  const m = new Map<string, unknown>();
  return {
    storage: {
      async get(key: string): Promise<unknown> { return structuredClone(m.get(key)); },
      async put(key: string, value: unknown): Promise<void> { m.set(key, structuredClone(value)); },
    },
  };
}

const deps = { Clock: { now: async (): Promise<number> => 1_000_000 } };

async function faultsAbsent(label: string, run: () => Promise<unknown>): Promise<void> {
  let message = "";
  try {
    await run();
  } catch (e) {
    message = String((e as { message?: string }).message ?? e);
  }
  assert(message.includes("key absent"), `${label} faults as key-absent, got: ${message || "no fault"}`);
}

const k = new Keys(fakeState() as never);
for (const name of ["constructor", "toString", "__proto__"]) {
  await faultsAbsent(`Map.update("${name}")`, () => k.bumpItem(name, {}));
  await faultsAbsent(`Cache.update("${name}")`, () => k.bumpLive(name, deps));
  await faultsAbsent(`indexed update("${name}")`, () => k.retag(name, "t", {}));
}
assert((await k.itemCount({})) === 0, "a faulted update stores nothing");

console.log("ALL OK");
"#;

#[test]
fn update_on_a_prototype_named_absent_key_faults() {
    verify(
        "update_absent",
        UPDATE_ABSENT_SOURCE,
        UPDATE_ABSENT_DRIVER_TS,
    );
}

// #1657 (runtime-semantics track §3.4): an `Int` stays a JS safe integer at
// runtime. A suite cannot assert a fault, so the driver catches each one:
//   - `Int` division by zero traps instead of producing `Infinity`/`NaN`;
//   - `round`/`floor`/`ceil`/`truncate` trap on a non-finite `Float` or one past
//     ±2^53 instead of producing an `Int` outside the domain;
//   - encoding an out-of-range `Int` (`Json.encode`, or the state commit)
//     faults instead of writing `null`, and a refused commit persists nothing;
//   - a stored `Int` outside the range faults on load with `RehydrationViolation`.
const INT_DOMAIN_SOURCE: &str = "context shop\n\
\n\
agent Calc {\n\
\x20 key id: String\n\
\x20 store last: Cell[Int] = 0\n\
\n\
\x20 on call div(a: Int, b: Int) -> Effect[Int] {\n\
\x20   a / b\n\
\x20 }\n\
\x20 on call conv(f: Float, how: String) -> Effect[Int] {\n\
\x20   if how == \"round\" { f.round() } else if how == \"floor\" { f.floor() } else if how == \"ceil\" { f.ceil() } else { f.truncate() }\n\
\x20 }\n\
\x20 on call encodeTimes(n: Int, k: Int) -> Effect[String] {\n\
\x20   Json.encode(n * k)\n\
\x20 }\n\
\x20 on call encodeNested(n: Int, k: Int) -> Effect[String] {\n\
\x20   let o: Option[Int] = Some(n * k)\n\
\x20   let xs: List[Int] = [1, n * k]\n\
\x20   Json.encode(o).concat(Json.encode(xs))\n\
\x20 }\n\
\x20 on call encodeList(n: Int, k: Int) -> Effect[String] {\n\
\x20   let xs: List[Int] = [1, n * k]\n\
\x20   Json.encode(xs)\n\
\x20 }\n\
\x20 on call store(n: Int, k: Int) -> Effect[()] {\n\
\x20   last := n * k\n\
\x20 }\n\
\x20 on call read() -> Effect[Int] {\n\
\x20   last\n\
\x20 }\n\
}\n";

const INT_DOMAIN_DRIVER_TS: &str = r#"
import { Calc } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

function fakeState() {
  const m = new Map<string, unknown>();
  return {
    storage: {
      async get(key: string): Promise<unknown> { return structuredClone(m.get(key)); },
      async put(key: string, value: unknown): Promise<void> { m.set(key, structuredClone(value)); },
    },
  };
}

async function faults(label: string, run: () => Promise<unknown>, needle: string): Promise<void> {
  let message = "";
  try {
    const v = await run();
    message = `no fault (got ${String(v)})`;
  } catch (e) {
    message = String((e as { message?: string }).message ?? e);
  }
  assert(message.includes(needle), `${label} faults with "${needle}", got: ${message}`);
}

const st = fakeState();
const c = new Calc(st as never);

// Division: ordinary values truncate, zero traps.
assert((await c.div(7, 2, {})) === 3, "7 / 2 truncates to 3");
assert((await c.div(-7, 2, {})) === -3, "-7 / 2 truncates toward zero");
await faults("5 / 0", () => c.div(5, 0, {}), "Int division by zero");
await faults("0 / 0", () => c.div(0, 0, {}), "Int division by zero");

// Float → Int conversions.
assert((await c.conv(2.5, "round", {})) === 3, "round");
assert((await c.conv(2.5, "floor", {})) === 2, "floor");
assert((await c.conv(2.1, "ceil", {})) === 3, "ceil");
assert((await c.conv(-2.7, "truncate", {})) === -2, "truncate");
for (const how of ["round", "floor", "ceil", "truncate"]) {
  await faults(`${how}(Infinity)`, () => c.conv(Infinity, how, {}), "is not a safe Int");
  await faults(`${how}(NaN)`, () => c.conv(NaN, how, {}), "is not a safe Int");
  await faults(`${how}(1e300)`, () => c.conv(1e300, how, {}), "is not a safe Int");
}

// Encode: in range is fine, out of range faults rather than writing null.
assert((await c.encodeTimes(3, 4, {})) === "12", "Json.encode of a safe Int");
await faults("Json.encode(2^53 * 4)", () => c.encodeTimes(9007199254740991, 4, {}), "safe-integer range");

// Nested: an Int inside an Option or a List goes through the same guard,
// via the generated instantiation codecs.
assert((await c.encodeNested(3, 4, {})) === '{"kind":"Some","value":12}[1,12]', "nested safe Ints encode");
await faults("Json.encode(Some(2^53 * 4))", () => c.encodeNested(9007199254740991, 4, {}), "safe-integer range");
await faults("Json.encode([1, 2^53 * 4])", () => c.encodeList(9007199254740991, 4, {}), "safe-integer range");

// Commit: an out-of-range Int is refused at encode, and nothing persists.
await c.store(5, 2, {});
assert((await c.read({})) === 10, "an in-range Int commits");
await faults("commit of 2^53 * 4", () => c.store(9007199254740991, 4, {}), "safe-integer range");
assert((await c.read({})) === 10, "the refused commit persisted nothing");

// Load: a stored Int outside the range faults on rehydration.
const bad = fakeState();
await bad.storage.put("state", { last: 1e300 });
await faults("load of a stored 1e300", () => new Calc(bad as never).read({}), "RehydrationViolation");

console.log("ALL OK");
"#;

#[test]
fn int_stays_a_safe_integer_at_runtime() {
    verify("int_domain", INT_DOMAIN_SOURCE, INT_DOMAIN_DRIVER_TS);
}

#[test]
fn store_held_connection_map_prototype_keys() {
    verify_with(
        "held_map",
        HELD_MAP_SOURCE,
        HELD_MAP_DRIVER_TS,
        bynkc::BuildTarget::Workers,
    );
}

// State-migrations S1 (#2211): stored state carries a `__v` schema-version
// marker. The lock below records `Counter` at version 1 with only `count`, so
// the source (which adds `label`) reconciles to version 2, readable back to 1.
const VERSIONED_SOURCE: &str = "context shop\n\
\n\
agent Counter {\n\
\x20 key id: String\n\
\x20 store count: Cell[Int] = 0\n\
\x20 store label: Cell[String]\n\
\n\
\x20 on call set(n: Int) -> Effect[()] {\n\
\x20   count := n\n\
\x20   Effect.pure(())\n\
\x20 }\n\
\x20 on call get() -> Effect[Int] {\n\
\x20   count\n\
\x20 }\n\
}\n";

const VERSIONED_LOCK_V1: &str = "version = 2\n\n[events]\n\n\
[agents.\"shop.Counter\"]\nversion = 1\nfloor = 1\n\n\
[[agents.\"shop.Counter\".shapes]]\nversion = 1\nadditive = false\n\n\
[[agents.\"shop.Counter\".shapes.fields]]\n\
name = \"count\"\nkind = \"Cell\"\ntype = \"Int\"\nrefinement = \"Int\"\n";

const VERSIONED_DRIVER_TS: &str = r#"
import { Counter } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

function fakeState(seed?: Record<string, unknown>) {
  const m = new Map<string, unknown>();
  if (seed !== undefined) m.set("state", structuredClone(seed));
  return {
    m,
    state: {
      storage: {
        async get(key: string): Promise<unknown> { return structuredClone(m.get(key)); },
        async put(key: string, value: unknown): Promise<void> { m.set(key, structuredClone(value)); },
      },
    },
  };
}

const stored = (f: { m: Map<string, unknown> }) => f.m.get("state") as Record<string, unknown>;

async function faultKind(run: () => Promise<unknown>): Promise<string> {
  try {
    await run();
  } catch (e) {
    const rv = (e as { rehydrationViolation?: { detail?: { kind?: string } } }).rehydrationViolation;
    return rv?.detail?.kind ?? `other: ${String(e)}`;
  }
  return "none";
}

const warnings: string[] = [];
const quietError = console.error;
console.warn = (...args: unknown[]) => { warnings.push(args.join(" ")); };
console.error = () => {};

// 1. A fresh key commits with the class's current version.
const fresh = fakeState();
await new Counter(fresh.state as never).set(5, {});
assert(stored(fresh).__v === 2, `a commit stamps __v = 2, got ${String(stored(fresh).__v)}`);

// 2. A record with no marker is version 1: it loads (all-additive to 2), the
//    added field reads its zero, and the next commit re-stamps it.
const legacy = fakeState({ count: 3 });
const lc = new Counter(legacy.state as never);
assert((await lc.get({})) === 3, "an unmarked record loads as version 1");
await lc.set(4, {});
assert(stored(legacy).__v === 2 && stored(legacy).count === 4, "the next commit re-stamps it");

// 3. A record from a newer build faults NewerSchema and is never written back.
const newer = fakeState({ count: 3, __v: 3 });
const nc = new Counter(newer.state as never);
assert((await faultKind(() => nc.get({}))) === "NewerSchema", "a newer record faults NewerSchema");
assert((await faultKind(() => nc.set(9, {}))) === "NewerSchema", "a write faults too");
assert(stored(newer).__v === 3 && stored(newer).count === 3, "the newer record is untouched");

// 4. A malformed marker has no path forward.
for (const bad of ["2", 0, -1, 1.5, null]) {
  const m = fakeState({ count: 3, __v: bad });
  const kind = await faultKind(() => new Counter(m.state as never).get({}));
  assert(kind === "NoMigrationPath", `__v = ${JSON.stringify(bad)} faults NoMigrationPath, got ${kind}`);
}

// 5. An undeclared stored key is reported once per isolate, and kept.
const orphan = fakeState({ count: 3, owner: "ada", __v: 2 });
const oc = new Counter(orphan.state as never);
await oc.get({});
await oc.get({});
await oc.set(6, {});
const ownerWarnings = warnings.filter((w) => w.includes("\"owner\""));
assert(ownerWarnings.length === 1, `one warning for "owner", got ${ownerWarnings.length}`);
assert(!ownerWarnings[0].includes("ada"), "the warning never carries the value");
assert(!warnings.some((w) => w.includes("\"__v\"")), "the marker itself is never reported");
assert(stored(orphan).owner === "ada", "the undeclared key is written back, not deleted");

console.error = quietError;
console.log("ALL OK");
"#;

#[test]
fn stored_state_is_stamped_and_checked_against_its_schema_version() {
    verify_with_lock(
        "schema_version",
        VERSIONED_SOURCE,
        VERSIONED_DRIVER_TS,
        Some(VERSIONED_LOCK_V1),
    );
}

// Decision A: a build with no schema lock emits no version logic, and carries
// a stored marker through untouched rather than faulting on it or restamping it.
const UNVERSIONED_DRIVER_TS: &str = r#"
import { Counter } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

const m = new Map<string, unknown>([["state", { count: 3, __v: 9 }]]);
const state = {
  storage: {
    async get(key: string): Promise<unknown> { return structuredClone(m.get(key)); },
    async put(key: string, value: unknown): Promise<void> { m.set(key, structuredClone(value)); },
  },
};
const c = new Counter(state as never);
assert((await c.get({})) === 3, "a lock-less build reads a marked record");
await c.set(4, {});
const s = m.get("state") as Record<string, unknown>;
assert(s.__v === 9 && s.count === 4, `the marker rides through untouched, got ${JSON.stringify(s)}`);
console.log("ALL OK");
"#;

#[test]
fn a_build_without_the_lock_carries_the_marker_through() {
    verify(
        "schema_version_off",
        VERSIONED_SOURCE,
        UNVERSIONED_DRIVER_TS,
    );
}

// State-migrations S2 (#2224): `migrate from N` steps at load. The lock is
// built the way a project's history builds it — v1, then an additive v2 — and
// v3 carries the non-additive change with a step from 2, which also covers v1
// (Decision C).
const MIGRATE_V1: &str = "context shop\n\
\n\
agent Cart {\n\
\x20 key id: String\n\
\x20 store owner: Cell[String]\n\
\x20 store total: Cell[Int]\n\
\x20 store items: Map[String, Int]\n\
\x20 store tags: Set[String]\n\
\n\
\x20 on call ping() -> Effect[Int] {\n\
\x20   1\n\
\x20 }\n\
}\n";

const MIGRATE_V2: &str = "context shop\n\
\n\
agent Cart {\n\
\x20 key id: String\n\
\x20 store owner: Cell[String]\n\
\x20 store total: Cell[Int]\n\
\x20 store items: Map[String, Int]\n\
\x20 store tags: Set[String]\n\
\x20 store note: Cell[String]\n\
\n\
\x20 on call ping() -> Effect[Int] {\n\
\x20   1\n\
\x20 }\n\
}\n";

const MIGRATE_V3: &str = "context shop\n\
\n\
type Name = opaque String where NonEmpty\n\
\n\
type CartV2 = { owner: String, total: Int, note: String = \"n/a\" }\n\
\n\
agent Cart {\n\
\x20 key id: String\n\
\x20 store ownerName: Cell[Option[Name]]\n\
\x20 store totalCents: Cell[Int]\n\
\x20 store share: Cell[Int]\n\
\x20 store items: Map[String, Int]\n\
\x20 store tags: Cell[Int]\n\
\x20 store memo: Cell[String]\n\
\n\
\x20 migrate from 2 (old: CartV2) {\n\
\x20   ownerName := Some(Name.unsafe(old.owner))\n\
\x20   totalCents := old.total * 100\n\
\x20   share := 600 / old.total\n\
\x20   memo := old.note\n\
\x20   drop tags\n\
\x20 }\n\
\n\
\x20 on call getOwner() -> Effect[Option[Name]] {\n\
\x20   ownerName\n\
\x20 }\n\
\x20 on call getCents() -> Effect[Int] {\n\
\x20   totalCents\n\
\x20 }\n\
\x20 on call getShare() -> Effect[Int] {\n\
\x20   share\n\
\x20 }\n\
\x20 on call getMemo() -> Effect[String] {\n\
\x20   memo\n\
\x20 }\n\
\x20 on call getTags() -> Effect[Int] {\n\
\x20   tags\n\
\x20 }\n\
\x20 on call itemCount() -> Effect[Int] {\n\
\x20   let n <- items.size()\n\
\x20   Effect.pure(n)\n\
\x20 }\n\
\x20 on call setCents(n: Int) -> Effect[()] {\n\
\x20   totalCents := n\n\
\x20   Effect.pure(())\n\
\x20 }\n\
}\n";

const MIGRATE_DRIVER_TS: &str = r#"
import { Cart } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

function fakeState(seed: Record<string, unknown>) {
  const m = new Map<string, unknown>([["state", structuredClone(seed)]]);
  return {
    m,
    state: {
      storage: {
        async get(key: string): Promise<unknown> { return structuredClone(m.get(key)); },
        async put(key: string, value: unknown): Promise<void> { m.set(key, structuredClone(value)); },
      },
    },
  };
}

const stored = (f: { m: Map<string, unknown> }) => f.m.get("state") as Record<string, unknown>;

async function fault(run: () => Promise<unknown>): Promise<string> {
  try {
    await run();
  } catch (e) {
    const rv = (e as { rehydrationViolation?: { detail?: { kind?: string } } }).rehydrationViolation;
    return rv?.detail?.kind ?? `other: ${String(e)}`;
  }
  return "none";
}

console.error = () => {};
console.warn = () => {};

// 1. A version-1 record (no marker) runs the step from 2 (its additive
//    predecessor): the old `note` it lacks takes CartV2's default, the
//    carried map survives, the re-kinded `tags` starts at its zero.
const v1 = fakeState({ owner: "ada", total: 3, items: { a: 1, b: 2 }, tags: { x: true } });
const c1 = new Cart(v1.state as never);
const own = (await c1.getOwner({})) as unknown as { tag: string; value?: string };
assert(own.tag === "Some" && own.value === "ada", `owner carried into ownerName: ${JSON.stringify(own)}`);
assert((await c1.getCents({})) === 300, "total * 100");
assert((await c1.getShare({})) === 200, "600 / total");
assert((await c1.getMemo({})) === "n/a", "a missing old field takes the old type's default");
assert((await c1.getTags({})) === 0, "a dropped, re-kinded field starts at its zero");
assert((await c1.itemCount({})) === 2, "an unchanged map is carried");
await c1.setCents(5, {});
const s1 = stored(v1);
assert(s1.__v === 3, `the next commit stamps the current version, got ${String(s1.__v)}`);
assert(!("owner" in s1) && !("total" in s1) && !("note" in s1), `old keys are gone: ${JSON.stringify(s1)}`);
assert(s1.memo === "n/a" && s1.totalCents === 5, JSON.stringify(s1));

// 2. A version-2 record keeps its own note.
const v2 = fakeState({ owner: "bo", total: 2, items: {}, tags: {}, note: "hi", __v: 2 });
assert((await new Cart(v2.state as never).getMemo({})) === "hi", "a version-2 note is read");

// 3. A key version 2 never knew is kept.
const extra = fakeState({ owner: "cy", total: 1, items: {}, tags: {}, note: "n", extra: 7, __v: 2 });
const ce = new Cart(extra.state as never);
await ce.setCents(1, {});
assert(stored(extra).extra === 7, "an unknown key survives a migration");

// 4. A step that throws is MigrationFailed, and the record is left alone.
const zero = fakeState({ owner: "di", total: 0, items: {}, tags: {}, note: "n", __v: 2 });
assert((await fault(() => new Cart(zero.state as never).getCents({}))) === "MigrationFailed", "division by zero in a step");
assert(stored(zero).total === 0 && stored(zero).__v === 2, "a failed step writes nothing");

// 5. A step whose output breaks a refinement faults at the ADR 0124 gate.
const blank = fakeState({ owner: "", total: 1, items: {}, tags: {}, note: "n", __v: 2 });
assert((await fault(() => new Cart(blank.state as never).getOwner({}))) === "RefinementViolation", "the gate validates a step's output");

// 6. A current record runs no step.
const cur = fakeState({ ownerName: { kind: "None" }, totalCents: 9, share: 1, items: {}, tags: 4, memo: "m", __v: 3 });
assert((await new Cart(cur.state as never).getTags({})) === 4, "a current record is read as it is");

console.log("ALL OK");
"#;

/// The lock a project reaches by building `history` in order, starting fresh.
fn lock_through(tag: &str, history: &[&str]) -> String {
    let dir = std::env::temp_dir().join(format!("bynk-store-lock-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).unwrap();
    let mut lock: Option<String> = None;
    for source in history {
        fs::write(dir.join("src/shop.bynk"), source).unwrap();
        let out = bynkc::compile_project(
            &bynk_testkit::compile_options_single(dir.join("src"))
                .target(bynkc::BuildTarget::Bundle)
                .schema_registry(bynkc::SchemaLock::On {
                    existing: lock.clone(),
                }),
        )
        .map_err(bynkc::ProjectFailure::flatten)
        .expect("each step of the history builds");
        lock = out.schema_lock;
    }
    let _ = fs::remove_dir_all(&dir);
    lock.expect("a lock-on build reconciles")
}

#[test]
fn migrate_steps_carry_stored_records_forward_at_load() {
    let lock = lock_through("migrate", &[MIGRATE_V1, MIGRATE_V2]);
    verify_with_lock("migrate", MIGRATE_V3, MIGRATE_DRIVER_TS, Some(&lock));
}

// #2243 review: a `Cell` holding a map is stored in the map value's own wire
// form (an entries array), unlike a store `Map`'s `Record`; a step reading it
// through `old` must decode it as such.
const CELL_MAP_V1: &str = "context shop\n\
\n\
agent Cart {\n\
\x20 key id: String\n\
\x20 store prices: Cell[Map[String, Int]] = Map.empty()\n\
\n\
\x20 on call ping() -> Effect[Int] {\n\
\x20   1\n\
\x20 }\n\
}\n";

const CELL_MAP_V2: &str = "context shop\n\
\n\
type CartV1 = { prices: Map[String, Int] }\n\
\n\
agent Cart {\n\
\x20 key id: String\n\
\x20 store costs: Cell[Map[String, Int]] = Map.empty()\n\
\n\
\x20 migrate from 1 (old: CartV1) {\n\
\x20   costs := old.prices\n\
\x20 }\n\
\n\
\x20 on call count() -> Effect[Int] {\n\
\x20   let c: Map[String, Int] = costs\n\
\x20   c.length()\n\
\x20 }\n\
}\n";

const CELL_MAP_DRIVER_TS: &str = r#"
import { Cart } from "./shop.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

const m = new Map<string, unknown>([["state", { prices: [["a", 1], ["b", 2]] }]]);
const state = {
  storage: {
    async get(key: string): Promise<unknown> { return structuredClone(m.get(key)); },
    async put(key: string, value: unknown): Promise<void> { m.set(key, structuredClone(value)); },
  },
};
const n = await new Cart(state as never).count({});
assert(n === 2, `a Cell holding a map migrates through old, got ${n}`);
console.log("ALL OK");
"#;

#[test]
fn a_migrate_step_reads_a_cell_holding_a_map() {
    let lock = lock_through("cell_map", &[CELL_MAP_V1]);
    verify_with_lock(
        "migrate_cell_map",
        CELL_MAP_V2,
        CELL_MAP_DRIVER_TS,
        Some(&lock),
    );
}
