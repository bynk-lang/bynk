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
- **Removing an agent** is refused with `bynk.agent.class_removed`.

If the agent has never been deployed, there is no data to protect: delete its
`[agents."<context>.<Agent>"]` entry from `bynk.schema.lock`, and the next build
records the new shape as a fresh start. If it has been deployed, widen rather than
narrow, or add a new field and move the data across in your own handlers. A
declared migration step is planned
([state-migrations track](https://github.com/bynk-lang/bynk/issues/2188)).

`bynkc check` and `bynkc test` check against the lock without changing it; a
build records each new shape.

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
