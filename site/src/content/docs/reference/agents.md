---
title: Agents
---
An agent is a keyed, stateful entity declared inside a `context`.

## Declaration

```bynk,fragment
agent Counter {
  key id: CounterId

  store count: Cell[Int]

  on call current() -> Effect[Int] {
    count
  }

  on call increment() -> Effect[Int] {
    let _ <- count.update((c) => c + 1)
    count
  }
}
```

| Part | Rule |
|---|---|
| `key <name>: <Type>` | the agent's identity; one key field. A handler reads it as `self.<name>`: the key this instance was addressed by. |
| `store <name>: <Kind>[…]` | a persistent field of a storage kind (`Cell`, `Map`, `Set`, `Cache`, `Log`). Every field needs an **initial value** — an explicit initialiser or an implicit zero (see below). |
| `on call <name>(…) -> Effect[T]` | a handler. The return type must be an `Effect` (`bynk.agent.return_not_effect`). |

A `Cell[T]` is a single stored value, read by its bare name and written with `:=`;
the other kinds (`Map`, `Set`, `Cache`, `Log`) expose effectful methods. See the
[storage kinds in the grammar reference](/reference/grammar/#rule-store_field) for the full
catalogue.

Agents may only be declared inside a context (`bynk.agent.outside_context`), and
may not declare HTTP handlers (`bynk.parse.handler_in_agent`).

## State initialisation

A never-seen key is initialised automatically, so every `store` field needs a
defined initial value. A field gets one in one of two ways:

**1. An explicit initialiser** — `store field: Cell[T] = <value>`. The value is a
compile-time constant: a literal (including one admitted to a refined type), a sum
variant, `Some`/`None`/`Ok`/`Err`, a record, or — for an opaque type —
`T.unsafe(lit)`. It may not reference `self`, parameters, or
capabilities (`bynk.agents.bad_state_initialiser` otherwise). An initialiser
makes any type admissible — including the ones that have no implicit zero.

```bynk,fragment
store status:  Cell[OrderStatus] = Pending   // a sum: the initial state
store level:   Cell[Level]       = 1         // a refined Int (Positive)
store retries: Cell[Int]         = 3         // a non-zero default
```

**2. An implicit zero** — a field with no initialiser must have a defined zero:

| Field type | Zero |
|---|---|
| `Int` | `0` |
| `Bool` | `false` |
| `String` | `""` |
| `Option[T]` | `None` |
| record of zeroable fields | each field zeroed |

A field that has neither an initialiser nor an implicit zero — an opaque type, a
non-`Option` sum, or a refined type that excludes its zero (`Int where
Positive`) — is rejected with
[`bynk.agents.non_zeroable_state_field`](/reference/diagnostics/bynk.agents.non_zeroable_state_field/).
Add an initialiser (or, to model "not set yet", use `Option[T]`). Collection kinds
with no implicit zero (such as `Cell[List[T]]`) likewise need an initialiser —
typically `= []`.

## State machines

Because a sum-typed `Cell` can carry an initial variant, an agent's state can be
a **state machine**: the sum's variants are the states, the initialiser names the
start state, `match <field>` reads the current state (exhaustively), and a
transition is an assignment:

```bynk,fragment
agent Order {
  key id: OrderId

  store status: Cell[OrderStatus] = Pending
  store items:  Cell[Int]

  on call place() -> Effect[Result[(), OrderError]] {
    match status {
      Pending => {
        status := Placed
        Ok(())
      }
      Placed    => Err(AlreadyPlaced)
      Cancelled => Err(AlreadyCancelled)
    }
  }
}
```

Bynk does not restrict which transitions are legal — any `:=` to any value of the
field's type type-checks. (Legal-transition tables are a later increment;
[invariants](/reference/agent-invariants/) constrain reachable states today.)

## Reading and writing state

- **Read** a `store` field by its bare name (`count`, `status`).
- **Write** a `Cell` with `name := <value>`. A `:=` is valid only against a
  `store Cell` field (`bynk.cell.invalid_target`); the value must match the cell's
  type; and a `:=` whose right-hand side names its own field is rejected
  (`bynk.cell.self_reference`) — a read-modify-write uses `update` instead.
- **Read-modify-write** a `Cell` with `update`, the one method-shaped cell
  operation:

  | Operation | Type | Notes |
  |---|---|---|
  | `cell.update(f)` | `Effect[()]` | `f: (T) -> T`, a pure combiner applied to the current value. Awaited with `<-`. Mutates the cell; does not return the new value (read the bare name back to observe it). |

  ```bynk,fragment
  let _ <- count.update((c) => c + 1)
  ```

  `read`/`write` are not callable methods — the bare name reads and `:=` writes.
  A read-modify-write made of separate calls (`get` then `put`) is not atomic across
  concurrent branches, so a `parForEach` or `parTraverse` closure may not write a store at all
  (`bynk.par.store_write_in_closure`); a single-call `update` or `upsert` run in a
  sequential `forEach` is atomic per key.
- **Commit** is implicit: every `store` write a handler makes is collected and
  persisted **atomically when the handler returns**, after invariants are checked.
  A handler that faults partway through persists nothing.

## Storage kinds and their operations

`Cell` is read by bare name and written with `:=`/`update`; the other four kinds
expose **effectful methods**, awaited with `<-`. The op is dispatched by receiver
provenance, so a `store` field's methods are the storage forms (the same type used
as a plain value keeps its pure-collection methods).

| Kind | Operations | Notes |
|---|---|---|
| `Map[K, V]` | `put`/`get`/`update`/`upsert`/`remove`/`contains`/`size` | `update` is for a key the handler knows exists, so it faults on an absent key: a missing key is a logic error, and the fault is its signal. `upsert` takes a default and creates the entry when the key is absent; `contains` tests first when the handler isn't sure |
| `Set[T]` | `add`/`remove`/`contains`/`size` | `add` is idempotent; `remove` of an absent member is a no-op |
| `Cache[K, V]` | the `Map` op set, with per-entry TTL expiry | requires `@ttl`; an expired entry reads as absent (check-on-read, needs `given Clock`) and is deleted when a handler that uses a `Cache` commits (at most 64 per commit, shared by all the agent's `Cache` fields) |
| `Log[T]` | `append`; lazy `Query` reads via `since`/`before`/`between`/`recent`/`reversed` | `append` stamps the time (`given Clock`); the window roots take explicit `Instant`s, so reads need no clock |

A `Map`'s or `Cache`'s key and a `Set`'s element may be any [`Key`](/reference/types/#keys)
type, as for a value `Map` or `Set`: a record, a sum, an `Option`, a `Decimal`, `Bytes`.
Two keys are the same entry exactly when they are `==`. A `String` or `Int` key (or a refined
or opaque type over one) is stored under its own text, as it always has been, so existing
state reads unchanged. Any other key is stored under its canonical encoding, which counts
towards the platform's 2 KiB limit on one storage key: an operation whose key encodes over it,
a read (`get`, `contains`) as much as a write, faults with a message naming the field, never the
key's value, as does an index posting whose indexed value and entry key together are over it. A type that is not a `Key`
(a `Float` at any depth, a `Map`, a `Set`, a function) is `bynk.store.unkeyable_key`. Changing
an existing field's key type from a scalar to a composite one is not an additive schema change,
and the schema lock refuses it. Nor is adding a field to a key record, even with a default: the
encoding is the stored key, so it would change every stored key and orphan the entries. A new
variant of a sum key is additive, and so is a new field in the map's value. An `@indexed(by:)` field may itself be any `Key` type, encoded the
same way.

```bynk
context shop

type StockKey = {
	region: String,
	code: Int,
}

agent Warehouse {
	key id: String

	store stock: Map[StockKey, Int]
	store blocked: Set[StockKey]

	on call add(k: StockKey, n: Int) -> Effect[()] {
		let _ <- stock.upsert(k, n, (q) => q + n)
	}
}
```

Every entry is subject to the [size limits](#size-and-cost-of-an-agents-state).

Reads over a `store Map`/`Log` are a lazy [`Query`](/reference/types/#query) — the same
combinator vocabulary the eager `List` carries, dispatched by provenance.

## Size and cost of an agent's state

An agent's state is persisted **per entry**: each entry of a `Map`, `Set`, `Cache` and
`Log`, and each posting of an [`@indexed`](#indexed-routing) field, is stored under a
Durable Object storage key of its own, while the `Cell` fields live together in one
state record beside the schema and layout markers. On the SQLite-backed
Durable Objects Bynk emits, one stored value is limited to about **8 MB** (key and
value combined; see Cloudflare's
[Durable Objects limits](https://developers.cloudflare.com/durable-objects/platform/limits/)),
so that limit now applies to a single entry, or to all the agent's `Cell`s taken
together, not to the whole state; an agent as a whole is bounded by the platform's
storage limit for one object instead. A handler writes only the entries it changed,
so the storage it writes is proportional to what it touches, not to the size of the
state.

Reads follow what a handler touches too. A call reads the `Cell` record at once, and
a collection only as the handler asks for it: `get`, `contains`, `put`, `update`,
`upsert` and `remove` of one key read that one entry; an equality filter on an
[`@indexed`](#indexed-routing) field reads the posting list for that value and the
entries it names; a `Log` window (`since`, `before`, `between`, `recent`) reads that
range of keys; `size` reads one count. A handler's writes stay in memory until it
returns and are committed with the `Cell` record in one transaction, so a handler
that faults writes nothing. What still reads a whole collection is a scan: `keys`,
`values` or `entries`, a filter that is not an `@indexed` equality, a join, a
group-by or an aggregate. Those read every entry in pages, so their cost still
grows with the size of the collection. Two scans stop as soon as the answer is known,
and read only the pages they need: `any` and `all` over a `Map` or a `Log`, and `first`,
`firstOrElse` and `take(n).collect()` over a `Log`, through any `filter`, `map` and
`flatMap` ahead of them (and `skip`, on a `Log`). A `Log` is read oldest first, so these
are the oldest matches. A `Map`'s scan order is insertion order, which only reading every
page recovers, so `first`, `firstOrElse` and `take` over a `Map` still read it whole. An
equality `filter` on an `@indexed` field reads that value's posting list, and `any` and `all` over it read the list a page at a time and stop at the answer; `first`, `firstOrElse` and `take` over it read the whole list, as the list is in key order. Each single operation on one key
(`update`, `upsert`, `put`, `remove`) is atomic and runs one after another per key, so
two concurrent branches' `update` or `upsert` calls on the same key both apply. A `get`
followed by a separate `put` is two operations, so the checker refuses a store write
written inside a `par` block or a `parTraverse`-family closure
([ADR 0797](https://github.com/bynk-lang/bynk/blob/main/design/decisions/0797-par-closure-store-write.md)):
use `update` or `upsert` for a read-modify-write, or move the write out of the
concurrent form.

A value stored the earlier way (the whole collection in one record), or a record a
`migrate from` step still covers, is read whole and split into entries by its next
write, in one transaction; `bynk migrate sweep` does that for every instance. An
agent with a `Map` of live connections (a held `Map`) also keeps its collection in
the record and reads it whole.

A stored value is decoded and checked when it is read, not when the call starts, so
an entry that no longer decodes (or a refined `Map` key or `Set` element that no
longer satisfies its refinement) faults with a rehydration violation when a handler
first reaches it, and a handler that never touches it does not.

Design for it:

- **Key agents finely.** One agent per customer, order or channel, not one per
  system: a collection that grows with your data belongs *across* agent instances,
  not inside one, because a scan, a join or a group-by still reads the whole of it.
- **Bound a `Log` with `@retain`**, which prunes old entries as you append. A
  `Cache` entry past its `@ttl` reads as absent, and the commit of any handler that
  uses a `Cache` deletes up to 64 expired entries in all (the limit is shared by
  every `Cache` the agent declares; oldest first, judged by the clock the handler
  read), so a `Cache` that is written regularly does not grow without bound. A
  `Cache` that nothing uses any more keeps its expired entries until a handler that
  uses it commits, and a large backlog drains 64 at a time. Keep a `Cache`'s key space bounded, or
  `remove` keys you are finished with.
- **Use D1 for a large or unbounded table.** A `Map` is the right home for a
  small, bounded collection owned by one agent; a table of every order, event or
  user is a database's job (see [`D1`](/reference/capabilities/)).

## Storage annotations

A `store` field may carry `@name(args)` annotations between the kind and the
initialiser. The vocabulary is a closed registry of five; an unknown name
(`bynk.store.unknown_annotation`) or a wrong-kind use
(`bynk.store.annotation_kind_mismatch`) is a diagnostic.

| Annotation | On | Meaning |
|---|---|---|
| `@ttl(<duration>)` | `Cache` | per-entry lifetime (required on a `Cache`; `bynk.store.cache_ttl_required`) |
| `@retain(<duration>)` | `Log` | prune entries older than the window on append |
| `@indexed(by: k)` | `Map` | maintain a secondary index keyed by `k` (see below) |
| `@unique(by: k)` | `Map` | a value of `k` may appear on at most one entry, enforced at commit (see below) |
| `@bounded(...)` | — | reserved |

### `@indexed` routing

`@indexed(by: k)` (v0.93, ADR 0118) declares a **runtime-maintained secondary
index** on a `store Map`. The runtime maintains it inside the same atomic commit,
and the compiler **routes an equality filter** through it: a query that filters the
map by equality on the indexed field becomes an index lookup rather than a full
scan, transparently — the query text is unchanged.

```bynk,fragment
store orders: Map[OrderId, Order] @indexed(by: customerId)
```

An equality filter on the indexed field is routed through the index:

```bynk,fragment
orders.filter((o) => o.customerId == c).collect()
```

Index hygiene is reported as **non-failing warnings** (the build still succeeds): a
query that filters by equality on an un-indexed field is `bynk.index.missing` (a
perf hint), and a declared `@indexed` that no equality filter uses is
`bynk.index.unused`.

### `@unique` {#unique}

`@unique(by: k)` declares that no two entries of a `store Map` hold the same value of the
value record's field `k`, which "no two customers share an email" needs and an
[invariant](/reference/agent-invariants/) cannot say, because a predicate may not scan a
collection. It implies `@indexed(by: k)`: the same index is maintained, equality filters on `k`
route through it, and declaring both is the warning `bynk.store.unique_implies_indexed`.

```bynk
context shop

type Customer = {
	id: String,
	email: String,
}

agent Directory {
	key id: String

	store customers: Map[String, Customer] @unique(by: email)

	on call add(c: Customer) -> Effect[Result[(), String]] {
		let taken <- customers.filter((x) => x.email == c.email).first()
		match taken {
			Some(_) => Err("taken"),
			None => {
				let _ <- customers.put(c.id, c)
				Ok(())
			},
		}
	}
}
```

The handler still checks first, so the caller sees `Err("taken")`; the constraint guarantees that a
handler that forgets cannot commit a duplicate. It is checked at commit, against the state the
handler staged: two puts of one value in one handler, and a put that collides with a committed
entry, refuse the commit, while moving a value from one entry to another within one handler is
allowed. The refusal is an `InvariantViolation` fault with the declaration `unique`, named
`<field>.<k>` (`customers.email`), and never carries the value. An `Option` field is unique among its
`Some` values: a `None` is not indexed, so any number of entries may hold one (an equality filter
on such a field is a scan, as the index is keyed by the payload).

**Adding `@unique` to a map that already holds entries.** The check reads the index, so the first
handler to touch the map after the change rebuilds its postings from its entries, in its own
commit (one pass over the map, once; the same happens when an index is added, removed or changes
its key scheme, as `@indexed` to `@unique` does on an `Option` field). Entries written before the
annotation are then found, and a put that collides with one is refused. Existing duplicates cannot
be checked at build time: a commit that touches a duplicated value is refused, and one that does
not is unaffected. [`bynk migrate sweep`](/reference/tooling/bynk-cli/#bynk-migrate-sweep) lists them:
it reports, for each `@unique` map, how many values more than one key holds and by how many keys each
is held, so you can clean them up before they refuse a commit.

`@unique` takes one `by:` field. `@unique(by: first, by: last)` is refused rather than read as a
constraint over the pair, which is not supported; a bare `@unique` is refused too.

## Invariants

An agent may declare **invariants** — predicates that must hold of every
committed state — in a phase between the `store` fields and the handlers:

```bynk,fragment
invariant available_non_negative:
  available >= 0
```

They are runtime-checked at each commit boundary; a violation faults before the
state is written. See [Agent invariants](/reference/agent-invariants/) for the predicate
surface (`implies`, `is`, pure value methods), the diagnostics, and what a caller
observes.

## Rehydration validation

An agent's persisted state is **stored in the wire shape and validated when it is
loaded** (v0.97; wire-shaped storage since #1649). A commit serialises each value
position — a `Cell`'s `T`, a `Map`/`Cache`'s `V`, a `Log`'s `T` — with the same
boundary codec the HTTP and queue seams use. A load runs each stored value, and
textual `Set` elements / `Map` keys, back through the boundary deserialiser, against
the **current** type definition. A failure is an internal
fault, **`RehydrationViolation`** — the load-time twin of an `InvariantViolation`
(it logs `RehydrationViolation <Agent>` with the agent, the field path and the
kind of failure, never the key/value) — *not* a caller-facing `400`:
the supplier of stored state is trusted past-self, not an untrusted caller.

Two consequences follow:

- A refinement that **tightens** across a deploy faults on load — orphaned data is
  indistinguishable from corruption — so breaking migrations stay by convention
  (no coercion, no silent drop).
- **Additive evolution is automatic:** a `store` field added in a later deploy
  takes its zero/initialiser instead of reading as absent. A stored field the
  current definition no longer declares is carried along unchanged, not dropped.
- A value with no wire form (a non-finite `Float`) cannot be stored: the commit
  faults and nothing persists.
- **Every stored record carries its schema version**, `__v`, the agent's version in
  `bynk.schema.lock`, written in the same commit. A record written by a *newer*
  build (after a rollback) faults on load as `RehydrationViolation` with detail
  `NewerSchema` and is never written back; a malformed marker faults with
  `NoMigrationPath`. A record carrying a `__layout` marker this build does not know
  (a later build wrote it) faults on load as `RehydrationViolation` with detail
  `NewerLayout` and is never written back. The build writes layout 2, which stores each
  entry of a `Map`, `Set`, `Cache` and `Log`, and each index posting, under its own key; an
  older record is split by its next commit, and `bynk migrate sweep` does that for every
  instance. A stored
  field the current definition no longer declares is
  logged once, by name only, as
  `bynk: agent <Agent> has stored field "<name>" the current definition does not declare; it is kept, not read`.

See the normative rule in
[§5.4.3 of the specification](/reference/spec/static-semantics/#543-rehydration-validation-v097).

### Changes that need no migration

`bynk.schema.lock` records each class's stored shape, and a build accepts a change
on its own when every record already stored still reads. That covers adding a
`store` field, changing only a refinement (which warns), and adding a record field
**with a default** anywhere inside a stored value:

```bynk
context shop

type Prefs = {
	theme: String,
	size: Int = 12,
}  // `size` added since the last deploy

agent Profile {
	key id: String

	store prefs: Cell[Prefs] = Prefs { theme: "light" }

	on call size() -> Effect[Int] {
		prefs.size
	}
}
```

A `Profile` stored before `size` existed loads with `size == 12`. The same holds
for a record that is a `Map`'s value, a `List`'s element or a `Log`'s entry.

A **new sum variant** anywhere inside a stored value is additive too, whatever its
payload: no value stored before it can name it. The change appears as, for example,
`last?|Paused variant added`, and it bumps the version. This is a rule about stored
data only; an exhaustive `match` over the sum still has to cover the new variant.
After a record holding the new variant is written, roll forward rather than back:
the older build faults the record as `NewerSchema`. This is the storage rule only. A
variant added to a type a service sends or receives is still a contract change,
because a caller built earlier cannot decode it.

Anything else changes what is on disk, and the build refuses it with
`bynk.agent.non_additive_state_change`, naming the position:
`one (one.dark added without a default)`, `prefs.theme removed`,
`prefs.size lost its default`. A lock written before the build recorded positions
judges a change inside a value as a retype until one build with the unchanged
definition fills them in (see [A lock written by an older toolchain](#a-lock-written-by-an-older-toolchain)).
A refused change needs a migration step, or one of the other fixes in
[`bynk.agent.non_additive_state_change`](/reference/diagnostics/bynk.agent.non_additive_state_change/).

### Reading a refused change

The error sits on the agent's name and lists what broke, grouped by cause:

| Clause | Cause |
|---|---|
| `field(s) removed: owner` | A `store` field is gone. |
| `field(s) retyped: prefs (…)` | A stored field's value changed. The parentheses name each position that broke. |
| `field(s) retyped: items` | The same, with no position: the lock predates positions. |
| `field(s) changed storage kind: n` | A field moved between storage kinds, such as `Cell` and `Map`. |

A renamed field is a removed field plus an added one, so it appears as `removed`.
When exactly one removed and one added field share a stored shape, a note names
them as a likely rename.

Each clause in parentheses is a path from the `store` field to the place that
changed, followed by what happened there:

| Clause | Meaning |
|---|---|
| `one (one.dark added without a default)` | A record field was added with no default, so a record stored before it cannot decode. |
| `prefs.theme removed` | A recorded record field, variant or element position no longer exists. |
| `prefs.size retyped` | The position is still there but now holds a different type. |
| `prefs.size lost its default` | A record field that had a default no longer has one, so a record stored without it would not decode. |
| `last?\|Paused removed` | A sum type lost a variant (a rename is a removal plus an addition), so a value stored with it would not decode. |

The path is built from these segments:

| Segment | Meaning | Example |
|---|---|---|
| `.field` | A record field, or a variant's payload field. | `prefs.theme`, `last?\|Added.sku` |
| `[]` | The element of a `List` or `Set`. | `many[]`, `many[].size` |
| `?` | The value inside an `Option`. | `last?` |
| `{}` | The value of a `Map` nested inside another type. | `index{}` |
| `\|Variant` | One variant of a sum type, or `\|Ok` / `\|Err` of a `Result`. | `last?\|Paused` |
| `#0`, `#1` | The first and second type argument of a storage kind that takes two. For `Map[K, V]`, `#0` is the key and `#1` the value. | `byId#1.size` |

The first segment is always the `store` field's name. A field with a single type
argument (`Cell[Prefs]`, `Log[Prefs]`) has no `#` segment. `bynk check` and
`bynk test` reconcile against the lock without writing it; only a build records a
new version.

### A lock written by an older toolchain

A `bynk.schema.lock` written before positions were recorded has no `paths` for its
fields. Against such a lock the first change inside a stored value is judged as a
retype of the whole field, and the error carries a note saying the lock predates
per-path shapes. Revert the change, build once so the lock records the positions
(the version does not change), then reapply it. A record field added with a
default is then accepted.

## Migrations

A `migrate from N (old: T) { … }` step carries a stored record from schema version
`N` (as `bynk.schema.lock` recorded it) to the agent's current shape. Declare steps
after the `store` fields, beside invariants, and before the handlers.

```bynk,fragment
type CartV1 = { owner: String, total: Int }

agent Cart {
  key id: String
  store ownerName: Cell[String]
  store totalCents: Cell[Int]

  migrate from 1 (old: CartV1) {
    ownerName := old.owner
    totalCents := old.total * 100
  }

  on call cents() -> Effect[Int] {
    totalCents
  }
}
```

| Part | Meaning |
|---|---|
| `from N` | A version the lock recorded below the current one. |
| `(old: T)` | A record type naming the version-`N` fields the step reads; checked against the lock on structure. Omit it for a step that reads nothing. Its fields may declare defaults. |
| `f := e` | Assigns a current `Cell` or `Map` field; at most once per field, in any order. |
| `let x = e` | A local binding for later lines. |
| `drop f` | Discards a stored field on purpose (a removed field, or a changed `Set`/`Log`/`Cache` field). |

- **What the body sees.** A bare field name reads its **carried** value: the stored
  value if the field's shape didn't change, else its zero. It never sees a value
  assigned earlier in the step.
- **Purity.** Steps are pure and cannot read the agent's key.
- **Coverage.** A step must account for every change from version `N` to now: read
  or drop each removed field, and assign each retyped one.
- **Older versions.** A step also covers older versions whose changes up to `N`
  were all additive. A field of `old` missing from such a record takes `T`'s field
  default, or its type's zero.
- **At load.** A record is migrated when it is next loaded, and written back in the
  new shape on its next commit.
- **Faults.** A step that throws faults as `RehydrationViolation` with detail
  `MigrationFailed`. A step whose output breaks a refinement faults at the
  rehydration gate.
- **Keep old types dedicated.** Declare a type like `CartV1` only for its step.
  Its field defaults describe what an old record lacked; a type's defaults apply
  wherever the type is built or decoded, so reusing it elsewhere (a request body,
  say) would bring those defaults along.
- **A map field.** A field of type `Map[K, V]` in `T` reads whatever version `N`
  recorded under that name: a `store Map`, or a `Cell` holding a map.

### The floor

Each class's entry in `bynk.schema.lock` records a **floor**: the oldest version
any stored instance can still hold. Only [`bynk migrate sweep`](/reference/tooling/bynk-cli/#bynk-migrate-sweep)
raises it, after it has woken every stored instance in every environment the
project deploys to. Raising it drops the lock's snapshots below it.

- **A step below the floor** (`migrate from N` with `N` below it) can no longer
  run, because no instance holds version `N`. It warns
  `bynk.agent.migrate_below_floor`, emits nothing, and can be deleted.
- **A step straddling the floor** keeps working for the versions at or above it.
- **No warning below the floor.** `bynk.agent.version_without_path` never warns
  about a version below the floor.

See [Change stored state safely](/book/guides/agents-and-state/stateful-agent/#change-stored-state-safely)
and [§5.4.3 of the specification](/reference/spec/static-semantics/#543-rehydration-validation-v097).

## Erasing an instance {#erase}

A handler deletes its own instance's stored state with `self.erase()`:

```bynk,fragment
agent Account {
	key id: String
	store email: Cell[Option[String]]
	store orders: Map[String, Order]

	on call close() -> Effect[()] given Events {
		do Events.emit[AccountClosed](AccountClosed { id: self.id })
		self.erase()
	}
}
```

When the handler commits, everything stored for the key is deleted: every field, every `Map`, `Set`,
`Cache` and `Log` entry, every `@indexed` posting, and the schema markers. The next call to the key
starts from the agent's **initial state**, as a key never seen does: each field reads its initialiser
or its zero. Nothing is left, so a closed account's data is gone and a finished order stops costing
storage.

- **The erasure is the commit.** The handler's other writes, before or after the call, are discarded,
  and a `:=` after `do self.erase()` in the same block is a warning (`bynk.agent.write_after_erase`),
  since it has nothing to write to.
- **It is atomic with the handler.** A handler that faults commits nothing, so it erases nothing, even
  if it called `self.erase()` first.
- **No invariant is checked** against the initial state the instance returns to. It is the state every
  new key starts in.
- **Events are still released.** Emit one first, as above, so a subscriber learns of the erasure and
  the erasure can be audited.
- **Only in an `on call` handler.** Not in a `migrate` step, an invariant or a function. To remove
  every instance of a class, see [Renaming and removing an agent](#renaming-and-removing-an-agent).
- **Tests** observe the reset by calling a read handler, as a user of the agent would
  ([guide](/book/guides/agents-and-state/delete-a-users-data/)).

## Data residency: `@jurisdiction` {#jurisdiction}

`@jurisdiction("eu")` (or `"fedramp"` or `"us"`) before `agent` (#3819) keeps every instance's state in that
Cloudflare jurisdiction:

```bynk,fragment
@jurisdiction("eu")
agent Learner {
	key id: String
	store total: Cell[Int]

	on call add(n: Int) -> Effect[Int] {
		let _ <- total.update((t) => t + n)
		total
	}
}
```

The values are the ones an `R2` bucket and a `D1` database take. The annotation is on the agent, and
there is no context-wide default. It is written once and takes one string literal; a second
`@jurisdiction`, a value that is not `"eu"`, `"fedramp"` or `"us"`, or an argument that is not a literal is
`bynk.agent.bad_annotation`. It combines with `@busyAfter` in either order.

An agent's calls, `bynk test` and the generated client are unchanged. The generated Worker reaches the
agent's Durable Object through `namespace.jurisdiction("eu")`, so every caller (a handler, a service,
a WebSocket route) mints the same restricted id. The Worker's configuration is unchanged: a
jurisdiction belongs to the id, not to the binding or the class.

**It cannot change after deploy.** An id minted through a jurisdiction-restricted namespace is a
different id from the unrestricted one for the same name, so adding, changing or dropping the
annotation on a deployed agent would orphan every existing instance, and nothing moves the data.
`bynk deploy` records the jurisdiction each class was pushed under in `bynk.deploy.lock`
(`agent_jurisdictions`), and a plan that differs prints

```text
change jurisdiction of durable object shop.Learner (none → eu; its existing instances are orphaned, irreversibly; needs --confirm-jurisdiction shop.Learner)
```

(`bynk deploy --dry-run` shows it and changes nothing). The deploy refuses until you confirm it by
name, at the prompt or with `--confirm-jurisdiction shop.Learner` (repeatable); `--yes` never stands
in, and a flag for a class whose jurisdiction does not change is refused, unless the ledger already
records the class (an earlier deploy applied the change, so a flag left in a CI script is harmless). A
deployed Worker whose ledger predates this record reads as unrestricted, which is what every agent
was, so adding `@jurisdiction` to an agent you already deployed asks for the confirmation. Once the
ledger records a Worker's classes, a class added later is new and asks for nothing, and a renamed
class (`agent New replaces Old`) is compared with the old class's jurisdiction, since a rename keeps
the instances.

**Locally.** `bynk dev` and `wrangler dev` run workerd, which does not implement jurisdictions: it
throws `Jurisdiction restrictions are not implemented in workerd`. Only on that exact message, the
generated Worker uses the unrestricted namespace and warns once per isolate. Any other failure
throws, so a deployed Worker can never silently lose the restriction. A jurisdiction is not visible
from Bynk, in a query or in `bynk test`. Cloudflare reports it inside the object, as `ctx.id.jurisdiction`
in a Durable Object (the value survives a round trip through the id's string form), which is where a live
check can read it.

See also [keep a project's data in one jurisdiction](/book/guides/projects-build-and-deployment/keep-data-in-a-jurisdiction/).

## Renaming and removing an agent

Once `bynk.schema.lock` records an agent class, it can't simply disappear from
source (`bynk.agent.class_removed`); declare what happens to its stored data.

| Declaration | Effect at the next deploy |
|---|---|
| `agent New replaces Old { … }` | `Old`'s stored data moves to `New`; the lock's history for `Old` (versions, shapes, `migrate` steps) becomes `New`'s. |
| `removed agent Old` (a context item) | `Old` and **all** of its stored data are deleted, irreversibly, after `bynk deploy` confirms it by name (`--confirm-delete <context>.Old`). |

- **The old name must be gone.** It can't still be declared as an agent
  (`bynk.agent.replaces_live`, `bynk.agent.removed_live`).
- **One fate per name.** Each old name has exactly one fate
  (`bynk.agent.class_conflict`).
- **A rename needs a recorded class.** `replaces` must name a class the lock records
  for this context (`bynk.agent.replaces_unknown`).
- **Remove the declaration once it's stale.** Keep each declaration until a deploy
  reports Cloudflare's `stale_tombstone` notice for it, then delete it.

## Renaming a store field {#rename-a-store-field}

A `Cell` store field is renamed with `replaces` after its type, naming the field's old name:

```bynk,fragment
agent Cart {
	key id: String
	store owner: Cell[String] replaces customer = ""

	on call owner() -> Effect[String] {
		owner
	}
}
```

Without the clause, renaming `customer` to `owner` is a removal plus an addition: deployed `customer`
values are not carried over, and the build is refused (`bynk.agent.non_additive_state_change`). With it:

- **The old key is read.** A record stored under `customer` loads as `owner`. When a record holds both
  (a stale `customer` beside an `owner` the new build wrote), `owner` wins. The first commit writes
  `owner` alone; `customer` is dropped.
- **The lock records it and the change is additive.** The class's version goes up by one, the new
  snapshot lists `owner` with `renamed_from = ["customer"]`, and the earlier snapshots keep
  `customer`, so a record stored at an earlier version is still read. A rename beside another additive
  change (a field added, a record field gained with a default) is judged by that change; beside a
  retype or a removal it is still refused.
- **It needs a recorded field.** `replaces` must name a field the lock records for this class
  (`bynk.field.replaces_unknown`), and not one the lock records beside the new name
  (`bynk.field.replaces_conflict`). It cannot name the field itself, a field the agent still declares, or a
  name another field replaces (`bynk.field.replaces_conflict`).
- **A `Cell`, a `Set` or a `Map`.** A `Map` or `Set` keeps each entry under a storage key that carries the
  field's name, so the first load after the deploy moves its entries to the new name, in batches, and then
  records that it has (a collection's cost grows with its size, and a crash part-way resumes on the next
  load). A `Cache`, a `Log` and a `Map` with `@indexed` are not renamed this way yet
  (`bynk.store.replaces_collection`): keep the collection's name, or move its entries with a
  [`migrate from` step](#migrations).
- **Keep the clause while a version needs it.** While any stored version the lock still holds keeps the
  value under the old name, deleting the clause would read those records as the field's zero, so the build is
  refused (`bynk.field.replaces_needed`). Once `bynk migrate` has raised the class's [floor](#the-floor) past
  them, the clause is no longer read, and the build warns (`bynk.field.replaces_settled`) that it can be
  deleted. A field renamed again keeps the whole history, so a record from before either rename is still read.

A rename carried by a `migrate from` step is unchanged: the step names the old shape's fields itself.
A record type used inside a `store` value is not renamed this way; a `replaces` on one of its fields is
read from the wire (see [Renaming a field](/reference/types/#field-replaces)) but the lock still judges the
stored shape by name, so change it with a `migrate` step. For the whole picture, see
[Rename a field safely](/book/guides/type-system/rename-a-field-safely/).

## Addressing and calling

Construct an agent with its key, then call a handler, binding the effect:

```bynk,fragment
let c = Counter(CounterId.unsafe("a"))
let n <- c.increment()
```

The same form works from a service handler or from another agent's handler. There
is no separate agent-reference type to declare.

### A call that fails — `.outcome(…)` {#outcome}

A failed agent call is a fault in the caller: a service that calls `Basket(id).add(item)`
cannot tell "busy, try again" from "the handler faulted", cannot answer differently, and its
`let … else` compensation does not run. Writing `.outcome` after the operation (#3933)
returns the failure as a value instead, the same `CallOutcome` a
[cross-context call](/reference/wire-format/#what-a-caller-makes-of-a-failure) returns:

```bynk,fragment
let o <- Basket(id).add.outcome(item)
match o {
	Handled(total) => Effect.pure(Ok(total))
	Unreachable(_) => Effect.pure(Err("basket busy, try again"))
	Faulted => Effect.pure(Err("could not add the item"))
	Rejected(_) => Effect.pure(Err("rejected"))
}
```

`Basket(id).add.outcome(item)` is `Effect[CallOutcome[R]]` for the handler's `R`, with the same
`o.handled()` for `?` and `embeds`. It is grammar on the operation reference, not a method of
`Effect`, so it is legal wherever the plain call is, an agent handler calling *another* agent
included. A call to the handler's own instance through `self` is part of the same handler and
stays a plain call.

| What happened to the agent call | The outcome |
|---|---|
| it answered | `Handled(result)` |
| `AgentBusy`: the call waited past the queue limit, so it never started | `Unreachable(None)` |
| a fault in the handler (it ran; it committed nothing) | `Faulted` |
| an invariant violation (it ran; it committed nothing) | `Faulted` |
| a rehydration violation (the stored state did not load) | `Faulted` |
| `ReentrantAgentCall` | stays a fault: a call cycle is a bug, and a value would invite retrying it |

`Rejected` never occurs for an agent call: there is no wire to refuse it. An invariant violation
is deliberately not a variant of its own: the [invariants](/reference/agent-invariants/) treat a
violation as a fault and a domain refusal as a `Result`, and a variant would invite using an
invariant as validation. A fault turned into a value is still logged once. Wrap an agent call
in `.outcome` only where a different answer or a compensation is needed.

At `unit`, nothing else makes an agent busy or fault on demand: a test
[stubs the operation](/reference/testing/#stub-agents), as in `stub Basket.add(_) unreachable`,
to drive the `Unreachable` and `Faulted` arms (#4215).

### One handler at a time

An agent instance runs one handler at a time: every handler that reaches an instance,
read-only ones included, waits for the handlers ahead of it, so a handler's load, body
and commit are never interleaved with another's on the same instance. Instances with
different keys run independently, and a handler that calls another handler on its own
agent through `self` is part of the same handler, not a second call. This is what makes
"persisted atomically when the handler returns" true when a handler awaits another
agent or a capability partway through.

Two faults keep the queue from hanging, and they are different faults:

- **`ReentrantAgentCall`**: a handler reached its own instance again, directly (an
  agent calling itself by key) or through other agents (`X` calls `Y`, which calls `X`).
  The call would wait on itself, so it faults at once, naming the agent. A call that
  leaves the Worker through a Service Binding and comes back is not seen by this check;
  it ends as `AgentBusy`.
- **`AgentBusy`**: a call waited 30 seconds behind the handlers ahead of it and gave up
  (`agent X busy: waited 30 s behind N queued calls`). The wait is the only thing timed;
  a handler may run for as long as the platform lets it. A hot instance can reach this with
  no cycle at all, which is why it is not reported as re-entrancy. It is transient load, so
  it is not answered as a bug: see below.

An agent can lower the 30-second wait with `@busyAfter(d)` before `agent` (#3932):

```bynk,fragment
@busyAfter(5.seconds)
agent Basket {
	key id: String

	store total: Cell[Int]

	on call add(n: Int) -> Effect[Int] {
		let _ <- total.update((t) => t + n)
		total
	}
}
```

`d` is a `Duration` literal from 1 second to 30 seconds (`bynk.agent.busy_after_out_of_range`
for any other; a constant naming one is allowed). It only lowers, never raises: a longer
wait only holds callers longer. It is per agent class, not per call, and any other
annotation on an agent is `bynk.agent.bad_annotation`.

What `AgentBusy` becomes depends on who made the call:

- **An HTTP route** answers `503 Service Unavailable` with `Retry-After: 1` instead of
  `500`. The hint is short and fixed: a queue wait is seconds, and clients should back off
  themselves. The fault is logged once, as `AgentBusy <Agent>` with the queue depth, and is
  not reported as a fault, so load does not raise a fault alert. The body is plain text
  until the service error format lands (#3916).
- **A cross-context call** reaches its caller as `503` with no fault marker, which
  `<context>.<service>.outcome(…)` reads as `Unreachable(Some(503))`: the call may not
  have run.
- **A queue consumer** faults, which retries the message; a **cron** run faults; an
  **event subscriber** faults (unchanged).
- A `ReentrantAgentCall` stays a fault and a `500` everywhere: a call cycle is a bug.

Two agents that call each other from separate roots (`A`'s handler calls `B` while `B`'s
handler calls `A`) wait on each other until both fault `AgentBusy`; the chain only sees a
call that is part of one chain. A handler that emits events gives its place up when it
hands the events to the fan-out, after its state is committed, so a subscriber that calls
the publisher back is served rather than queued behind it.

In the in-memory bundle target (`bynk test` without a Workers runtime) calls are queued
the same way, but a cycle is only caught by the wait limit, not named as
`ReentrantAgentCall`.

## Capabilities a handler needs

An agent handler declares the capabilities its body uses with `given`, exactly
as a service handler does — `on call put(...) -> Effect[()] given Clock`. Some
requirements are **implied** rather than written: a `store Cache` op applies TTL
expiry and a `store Log` `append` stamps the time, so both read the clock and the
handler must declare `given Clock` even though nothing in the body names it
(`bynk.store.cache_needs_clock` / `bynk.store.log_needs_clock`).

Because such a requirement is invisible at the signature, the editor surfaces it:
on a handler whose body needs a capability its `given` does not cover, a **ghost
clause** is shown after the return type —

```text
on call put(token: Token, value: V) -> Effect[()]  «given Clock»
```

— and accepting the hint writes the real `given Clock` in place. The reason is
derived from where the requirement arises (a store op, or a direct `Cap.op(...)`
call), so it works for any capability, including your own.

An agent `on call` handler carries **no `by` clause** — `by` establishes the
actor from an inbound request at a service edge, and an agent is reached across
the agent boundary, not from an ingress. A `by` on an agent handler is rejected
(`bynk.actor.by_on_agent`).

## Lifecycle and emission

A fresh key's state falls back to the compiled zero value on first access. On the
`bundle` target an agent uses an in-process state registry; on `workers` it
compiles to a Cloudflare Durable Object keyed by the agent key. See
[emission](/reference/tooling/emission/) and [The agent model](/book/concepts/agents-and-state/the-agent-model/).
