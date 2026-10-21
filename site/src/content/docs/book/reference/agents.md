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
[storage kinds in the grammar reference](/book/reference/grammar/#rule-store_field) for the full
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
[`bynk.agents.non_zeroable_state_field`](/book/troubleshooting/agents-non-zeroable-state-field/).
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
[invariants](/book/reference/agent-invariants/) constrain reachable states today.)

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
| `Map[K, V]` | `put`/`get`/`update`/`upsert`/`remove`/`contains`/`size` | `update` on an absent key faults — use `upsert` for default-if-absent |
| `Set[T]` | `add`/`remove`/`contains`/`size` | `add` is idempotent; `remove` of an absent member is a no-op |
| `Cache[K, V]` | the `Map` op set, with per-entry TTL expiry | requires `@ttl`; eviction is lazy, check-on-read, and needs `given Clock` |
| `Log[T]` | `append`; lazy `Query` reads via `since`/`before`/`between`/`recent`/`reversed` | `append` stamps the time (`given Clock`); the window roots take explicit `Instant`s, so reads need no clock |

A `Map`'s or `Cache`'s key and a `Set`'s element must be **value-keyable**: `String`,
`Int`, or a refined or opaque type over them, as for a value `Map` key
(`bynk.store.unkeyable_key`, #1680). A store collection is keyed by the key's
string form, so a record has no stable key: key by one of its id fields instead.

Reads over a `store Map`/`Log` are a lazy [`Query`](/book/reference/types/#query) — the same
combinator vocabulary the eager `List` carries, dispatched by provenance.

## Storage annotations

A `store` field may carry `@name(args)` annotations between the kind and the
initialiser. The vocabulary is a closed registry of four; an unknown name
(`bynk.store.unknown_annotation`) or a wrong-kind use
(`bynk.store.annotation_kind_mismatch`) is a diagnostic.

| Annotation | On | Meaning |
|---|---|---|
| `@ttl(<duration>)` | `Cache` | per-entry lifetime (required on a `Cache`; `bynk.store.cache_ttl_required`) |
| `@retain(<duration>)` | `Log` | prune entries older than the window on append |
| `@indexed(by: k)` | `Map` | maintain a secondary index keyed by `k` (see below) |
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

## Invariants

An agent may declare **invariants** — predicates that must hold of every
committed state — in a phase between the `store` fields and the handlers:

```bynk,fragment
invariant available_non_negative:
  available >= 0
```

They are runtime-checked at each commit boundary; a violation faults before the
state is written. See [Agent invariants](/book/reference/agent-invariants/) for the predicate
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
  `NoMigrationPath`. A stored field the current definition no longer declares is
  logged once, by name only, as
  `bynk: agent <Agent> has stored field "<name>" the current definition does not declare; it is kept, not read`.

See the normative rule in
[§5.4.3 of the specification](/book/spec/static-semantics/#543-rehydration-validation-v097).

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
  Because it may declare field defaults, using it anywhere else (a request body,
  say) would apply those defaults at that boundary too.
- **A map field.** A field of type `Map[K, V]` in `T` reads whatever version `N`
  recorded under that name: a `store Map`, or a `Cell` holding a map.

### The floor

Each class's entry in `bynk.schema.lock` records a **floor**: the oldest version
any stored instance can still hold. Only [`bynk migrate sweep`](/docs/bynk-cli/#bynk-migrate-sweep)
raises it, after it has woken every stored instance in every environment the
project deploys to. Raising it drops the lock's snapshots below it.

- **A step below the floor** (`migrate from N` with `N` below it) can no longer
  run, because no instance holds version `N`. It warns
  `bynk.agent.migrate_below_floor`, emits nothing, and can be deleted.
- **A step straddling the floor** keeps working for the versions at or above it.
- **No warning below the floor.** `bynk.agent.version_without_path` never warns
  about a version below the floor.

See [Change stored state safely](/book/guides/agents-and-state/stateful-agent/#change-stored-state-safely)
and [§5.4.3 of the specification](/book/spec/static-semantics/#543-rehydration-validation-v097).

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

## Addressing and calling

Construct an agent with its key, then call a handler, binding the effect:

```bynk,fragment
let c = Counter(CounterId.unsafe("a"))
let n <- c.increment()
```

The same form works from a service handler or from another agent's handler. There
is no separate agent-reference type to declare.

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
[emission](/docs/emission/) and [The agent model](/book/guides/agents-and-state/the-agent-model/).
