---
title: Build a stateful agent and keep its state zeroable
---
**Goal:** declare an agent that owns state, reads it, and updates it — with state
that initialises cleanly for a never-seen key.

Agents live inside a `context`.

## Declare the agent

Give it a `key` (its identity), one or more `store` fields, and handlers:

```bynk
context counters

type CounterId = opaque String

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

- Read a `store` field by its bare name (`count`).
- Write it unconditionally with `:=` (the new value does **not** depend on the old
  one).
- When the new value *does* depend on the old one, use `count.update(fn)` — a
  read-modify-write — rather than `:=`. A `:=` whose right-hand side names its own
  field is rejected (see [below](#modify-a-cell-in-place)).
- Every `store` write is committed atomically when the handler returns; there is
  no `commit` step, and a faulting handler persists nothing.
- Handlers return `Effect[T]`; returning a plain value in tail position is lifted
  automatically.

## Modify a cell in place

A `:=` write replaces a cell's value with an expression that stands on its own:

```bynk,fragment
count := 0          // reset
limit := limit      // rejected: the right-hand side reads the cell being written
```

When the new value is computed *from the old one*, reach for `update(fn)` instead.
It takes a pure combiner `(T) -> T` and applies it to the current value:

```bynk,fragment
let _ <- count.update((c) => c + 1)   // increment
let _ <- count.update((c) => c * 2)   // double
```

Why a separate operation rather than `count := count + 1`? Because the latter
hides a *read* of the prior value inside what looks like a plain write. Making it
`update` keeps that prior-value dependency visible (and the combiner retry-safe).
A self-referencing `:=` is therefore rejected with
[`bynk.cell.self_reference`](/book/reference/diagnostics/), steering you to
`update`.

`update` mutates the cell; it does not return the new value. To read-modify-write
**and** return — as `increment` above does — await the `update`, then read the
bare name back (the read sees the staged write):

```bynk,fragment
let _ <- count.update((c) => c + 1)
count                                  // the committed new value
```

## Keep state zeroable

Every `store` field needs a starting value for the never-seen key that Bynk
initialises automatically. Either the type has a zero (`Int`→`0`, `Bool`→`false`,
`String`→`""`, `Option[T]`→`None`), or you supply an explicit initialiser with
`=`. A field whose type excludes its zero (for example a refined
`type Limit = Int where Positive`, which excludes `0`) and which has no
initialiser is rejected with
[`bynk.agents.non_zeroable_state_field`](/book/troubleshooting/agents-non-zeroable-state-field/).

When you need "not set yet", use `Option`:

```bynk,fragment
store reading: Cell[Option[Int]]   // starts as None — "never set"
```

When the type has no zero but you have a sensible default, give an initialiser:

```bynk,fragment
store limit: Cell[Limit] = 1
```

A store type argument names a type, so declare the refinement as its own type
(`type Limit = Int where Positive`) rather than writing it inline.

## Beyond `Cell`: maps, sets, caches, and logs

`Cell` holds a single value; the other four storage kinds hold collections and
expose **effectful methods** (awaited with `<-`) instead of `:=`.

A **`Map`** keys values; a **`Set`** holds membership:

```bynk,fragment
store members: Set[UserId]
store profiles: Map[UserId, Profile]

on call join(u: UserId, p: Profile) -> Effect[()] {
  let _ <- members.add(u)         // idempotent
  let _ <- profiles.put(u, p)
  ()
}

on call lookup(u: UserId) -> Effect[Option[Profile]] {
  let found <- profiles.get(u)
  found
}
```

A **`Cache`** is a TTL-bounded map: `@ttl` is required, and any time-consulting op
needs `given Clock` (which makes expiry testable with a mocked clock):

```bynk,fragment
store sessions: Cache[SessionId, Session] @ttl(30.minutes)

on call touch(id: SessionId, s: Session) -> Effect[()] given Clock {
  let _ <- sessions.put(id, s)    // expires 30 minutes after it is written
  ()
}
```

A **`Log`** is an append-only, time-indexed sequence. `append` stamps the current
time (so it needs `given Clock`), but the window reads take explicit `Instant`s and
so need no clock — they return a lazy [`Query`](/book/reference/types/#query):

```bynk,fragment
store events: Log[Event] @retain(7.days)

on call add(e: Event) -> Effect[()] given Clock {
  let _ <- events.append(e)
  ()
}

on call recent_count(since: Instant) -> Effect[Int] {
  let n <- events.since(since).count()
  n
}
```

A map query yields values by default, but `map.entries` exposes each entry as a
[`MapEntry[K, V]`](/book/reference/types/#map-key-accessors) record — read the key
with `.key` and the value with `.value`, so a stored value need not carry a
denormalised copy of its own key (`map.keys` and `map.values` give the keys and
values alone):

```bynk,fragment
store items: Map[String, Stored]

on call all() -> Effect[List[TodoItem]] {
  items.entries.map((e) => TodoItem { id: e.key, seq: e.value.seq, title: e.value.title, done: e.value.done }).collect()
}
```

To route an equality filter through a maintained index, annotate the map with
[`@indexed`](/book/reference/agents/#indexed-routing).

## Change stored state safely

A deployed agent's `store` fields hold data your users wrote, so a build records
each agent's stored shape in `bynk.schema.lock` (commit it with your source) and
refuses a change that deployed data cannot be read through.

- **Adding a field** is safe. Instances stored before it existed read it as its
  zero or initialiser.
- **Removing, retyping, or changing the storage kind of a field** is refused with
  `bynk.agent.non_additive_state_change`. So is **renaming** one: the old values
  would be left behind under the old name while the new field reads as zero. When
  exactly one field disappeared and one with the same shape appeared, the
  diagnostic names the likely rename.
- **Changing a refinement** builds, with a warning
  (`bynk.agent.refinement_changed`). Widening one is safe. Narrowing one makes any
  stored value that no longer fits fault on load, so migrate that data first.
- **Removing or renaming an agent** without saying so is refused with
  `bynk.agent.class_removed`. Declare what happens to its data instead:
  - `agent Basket replaces Cart { … }` renames the class. Its stored data moves to
    `Basket` when you deploy, and its schema history (and any `migrate` steps) moves
    with it.
  - `removed agent Wishlist`, at the top level of the context, deletes the class
    and **all of its stored data** when you deploy. The deploy asks you to confirm
    each deletion by name (`bynk deploy --confirm-delete shop.Wishlist`).

  Keep the declaration until a deploy prints Cloudflare's `stale_tombstone` notice
  for it, then delete it. To rename a class twice, deploy the first rename before
  writing the second.

To make such a change, declare a **migration step** that carries the stored data
across. Rename `owner` to `ownerName`:

```bynk,fragment
type CartV1 = { owner: String }

agent Cart {
  key id: String
  store ownerName: Cell[String]

  migrate from 1 (old: CartV1) {
    ownerName := old.owner
  }

  on call owner() -> Effect[String] {
    ownerName
  }
}
```

- **`old` declares what you read.** `CartV1` is an ordinary record type naming the
  fields of version 1 that the step reads. The build checks it against the version
  `bynk.schema.lock` recorded, so it can't drift from what is stored.
- **The body assigns current fields.** Each `f := e` gives a current field its
  migrated value. Each field is assigned at most once, the order doesn't matter, and
  a bare field name reads the value carried over from the old record, never one
  assigned earlier in the step. A field you leave alone is carried over if its
  shape didn't change.
- **Every change must be accounted for.** A removed field must be read (`old.f`) or
  dropped with `drop f`. A retyped field must be assigned. A `Set`, `Log` or `Cache`
  field can't be read or assigned in a step, so a change to one is covered by
  `drop f`, which starts it afresh.
- **Steps are pure.** No capabilities, no effects, and no `key`. A step that throws
  while running, such as an `Int` division by zero, faults on load as
  `MigrationFailed`, and the stored record is left alone. The step's output still
  has to satisfy your types: a refinement it breaks faults on load like any other.
- **One step covers older versions too.** A step `from 2` also handles a record
  from version 1 if every change from 1 to 2 was additive. A field of `old` that
  such a record lacks takes the default declared on the old type
  (`note: String = "n/a"`), or its type's zero.

A stored record is migrated when it next loads, and written back in the new shape
the next time a handler changes it. A version that no step covers and that can't be
read forward warns with `bynk.agent.version_without_path`; a record at that version
faults with `NoMigrationPath` if one is ever loaded.

If the agent has never been deployed, there is no data to protect: you can instead
delete its `[agents."<context>.<Agent>"]` entry from `bynk.schema.lock`, and the
next build records the new shape as a fresh start.

`bynkc check` and `bynkc test` check against the lock without changing it; a
build records each new shape.

Every record an agent stores carries that version, as a `__v` field written with
it. Two things follow:

- **Rolling back is loud.** If you redeploy an older build after a newer one has
  written state, an agent whose record is newer than the code faults on load
  (`NewerSchema`) instead of misreading it, and leaves it untouched. Redeploy
  forward again to recover.
- **Re-baselining resets the version to 1.** Delete a class's lock entry only if
  it was never deployed: a deployed instance's records would be newer than the
  fresh baseline, and would fault. (Local `bynk dev` state needs no care: `bynk dev`
  starts every session with empty storage.)

A field your agent no longer declares, renamed before the lock recorded it, is
logged once by name when an agent loads it; its value is kept.

## Address an agent

Construct an agent with its key, then call a handler (binding the effectful
result with `<-`):

```bynk,fragment
let c = Counter(CounterId.unsafe("a"))
let n <- c.increment()
```

## Related

- Tutorial: [Add a stateful agent](/book/tutorials/05-stateful-agent/).
- Reference: [agents](/book/reference/agents/).
- Troubleshooting: [`bynk.agents.non_zeroable_state_field`](/book/troubleshooting/agents-non-zeroable-state-field/).
