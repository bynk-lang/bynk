---
title: Type system
---
## Built-in base types

| Type | Values | Emits |
|---|---|---|
| `Int` | integer literals (`0`, `-42`); a JS safe integer, ±(2^53 − 1) | `number` |
| `Float` | float literals (`1.5`, `0.0`, `-3.14`) | `number` |
| `String` | string literals (`"…"`) | `string` |
| `Bool` | `true`, `false` | `boolean` |
| `Duration` | unit literals (`5.minutes`, `30.seconds`) | `number` (millis) |
| `Instant` | no literal — `Clock.now()` / `Instant.fromEpochMillis(n)` / `Instant.fromIso(s)` | `number` (epoch millis) |
| `Bytes` | no literal — `Bytes.fromUtf8(s)` / `Bytes.fromBase64(s)` / `Bytes.fromHex(s)` / `Bytes.fromBase64Url(s)` / `Bytes.empty()` | `Uint8Array` |

The unit type is written `()`. `Int` and `Float` are **distinct and
incompatible** — there is no implicit coercion (`bynk.types.no_numeric_coercion`).
Convert explicitly: `i.toFloat()` (Int → Float, total) or `f.round()` /
`f.floor()` / `f.ceil()` / `f.truncate()` (Float → Int, a runtime fault when the
result is not a safe integer); parse a string with `Int.parse(s)` /
`Float.parse(s)`, each returning `Option` and accepting a strict full-string
decimal only (`" 7 "`, `"0x10"` and, for `Int`, `"1e3"` are `None`). An `Int`
is a JS safe integer at every entry point: a larger literal, a wire value, a
parse result or a conversion is rejected, and `Int` division by zero faults.

### Duration

`Duration` (v0.86) is a **span of time**, erased to a `number` of milliseconds. A
`Duration` literal is `<int>.<unit>` over a closed unit set — `5.minutes`,
`30.seconds`, `1.hours`, `2.days`, `100.milliseconds`. The operator surface is
`Duration ± Duration`, `Duration * Int` / `Int * Duration` (scalar scaling), and
`Duration` comparison (subtraction is unclamped — may go negative). Convert
explicitly: `d.toMillis() -> Int` and the static `Duration.millis(n: Int) ->
Duration`. It round-trips through the JSON codec as an integer. See
[Operators](/book/reference/operators/#duration--instant-arithmetic).

### Instant

`Instant` (v0.90) is an **absolute point in time**, erased to a `number` of Unix
epoch milliseconds. It has **no literal**: an `Instant` is minted by `Clock.now()`
(typed `Effect[Instant]`) or built from an `Int` via `Instant.fromEpochMillis(n)`.
Arithmetic composes with `Duration`: `Instant ± Duration -> Instant`
(advance/retreat) and `Instant - Instant -> Duration` (the span between).
Comparison is chronological and `Instant` is **orderable** (so `sortBy`/`min`/`max`
key on it) but **not numeric** (`sum`/`average` reject it). The escape to raw
millis is `t.toEpochMillis() -> Int`; the zero is the epoch. Timestamp math goes
**through `Instant`** — `now + 5.minutes` is `Instant + Duration`; the former
`Int + Duration -> Int` clock-math coercion was withdrawn at v0.90, so every
`Instant`↔`Int` mix is a `no_numeric_coercion` error. See
[Operators](/book/reference/operators/#duration--instant-arithmetic).

#### ISO-8601 text

`t.toIso() -> String` writes an instant as RFC 3339 text in UTC, always
`YYYY-MM-DDTHH:mm:ss.sssZ`: three fraction digits and `Z`, so the text sorts in
time order. Outside the years 0000 to 9999 the year takes the expanded `±YYYYYY`
form, and an instant beyond ±8.64e15 ms (past any date) faults. `toString()`
still gives the epoch milliseconds, and the JSON codec still writes a number.

`Instant.fromIso(s) -> Option[Instant]` reads the RFC 3339 date-time profile:

```bynk,fragment
let utc = Instant.fromIso("2026-10-30T08:54:50.123+02:00")   // Some(…06:54:50.123Z)
let bad = Instant.fromIso("2026-10-30 08:54:50Z")             // None: a space, not `T`
```

- `YYYY-MM-DD`, `T` (or `t`), `HH:mm:ss`, an optional fraction of any length,
  then `Z` (or `z`) or an offset `±HH:mm`, converted to UTC. `-00:00`
  ("offset unknown", RFC 3339) reads as `Z`: the time written is UTC.
- A fraction past milliseconds is truncated toward the earlier instant.
- The date must exist (`2026-02-29` is `None`, `2028-02-29` is not), and a leap
  second (`:60`) is `None`, since epoch milliseconds cannot name it.
- Anything else is `None`: a space instead of `T`, a time with no offset (it
  names no instant without a zone), the basic form without separators, week and
  ordinal dates, and the expanded year.

`Instant.fromIso(t.toIso())` is `Some(t)` for every instant in the years 0000 to
9999. Time zones, calendar fields and ISO durations are not part of the kernel.

### Bytes

`Bytes` (v0.110) is an **immutable octet sequence** — the type for arbitrary
binary data that `String` (UTF-8 text) cannot hold without corruption. It is the
one base type that does **not** emit to `number`: a `Bytes` erases to a host
`Uint8Array`. There is **no literal**; construct a `Bytes` with:

- `Bytes.fromUtf8(s: String) -> Bytes` — the UTF-8 encoding of a string (total).
- `Bytes.fromBase64(s: String) -> Option[Bytes]` — decode base64; `None` on an
  invalid string.
- `Bytes.fromHex(s: String) -> Option[Bytes]` — decode hex. Either case, an even
  number of digits and nothing else: a `0x` prefix, whitespace or separators give
  `None`. `""` decodes to `Bytes.empty()`.
- `Bytes.fromBase64Url(s: String) -> Option[Bytes]` — decode base64url (RFC 4648
  §5, the `-`/`_` alphabet), with no padding or exactly the padding the length
  needs. A standard-alphabet `+` or `/`, wrong padding, an impossible length or
  non-zero spare bits in the last character gives `None`, so each value has exactly
  one accepted spelling (`"Zg"` decodes, `"Zh"` doesn't).
- `Bytes.empty() -> Bytes` — the zero value (the empty sequence).

The methods:

| Method | Returns | |
|---|---|---|
| `b.length()` | `Int` | the octet count |
| `b.concat(other: Bytes)` | `Bytes` | `b`'s octets, then `other`'s |
| `b.slice(start: Int, end: Int)` | `Bytes` | clamped like `String.slice`: a negative index is `0`, an end past the length is the length, and `start >= end` is empty |
| `b.compare(other: Bytes)` | `Int` | `-1`, `0` or `1` in unsigned lexicographic order; a proper prefix sorts first |
| `b.toHex()` | `String` | lowercase, two digits per octet |
| `b.toBase64()` | `String` | standard base64, padded |
| `b.toBase64Url()` | `String` | base64url, unpadded (the JWT form) |
| `b.decodeUtf8()` | `Option[String]` | `None` on an invalid UTF-8 sequence |

Encoding is total; decoding is partial and surfaced as `Option`, never hidden.
Every decoder round-trips its encoder: `Bytes.fromHex(b.toHex())` is `Some(b)`
for every `b`, and likewise for base64 and base64url.

`compare` and `==` are **not constant-time**: they stop at the first differing
octet, so their timing reveals how much of two values matches. Never use them to
check a signature, MAC or token against a secret: use `Crypto.equal` from
[`bynk.crypto`](/book/reference/bynk-capabilities/#cryptography--bynkcrypto),
which compares in constant time.

`==`/`!=` compare **by content**, byte for byte — so two independently-built
`Bytes` with the same octets are equal (unlike the number-erased base types, a
`Bytes` is not compared by host reference). Equality is structural throughout,
so a record carrying a `Bytes` field compares that field by content too. `Bytes` is **not orderable** (no `<` / `sortBy` key; use
`b.compare(other)`) and **not `Map`-keyable** — key on `b.toBase64()` (a
`String`) instead. It has no arithmetic.

On the wire a `Bytes` **serialises as a base64 JSON string** (and deserialising
requires a valid base64 string), so it round-trips through any record or `store`
field and crosses a `bundle` context boundary — a fully ordinary serialisable
value, the opposite of a `Stream`. (One current limit: a bare `Bytes` directly in
a `workers` cross-context signature is diagnosed as not-yet-supported — put it
inside a record, whose typed codec base64-encodes it, or build with `--target
bundle`.)

## Built-in generic types

| Type | Variants | Purpose |
|---|---|---|
| `Result[T, E]` | `Ok(T)`, `Err(E)` | success or error |
| `Option[T]` | `Some(T)`, `None` | a value or nothing |
| `Effect[T]` | — | an effectful computation yielding `T` |
| `HttpResult[T]` | see [HTTP](/book/reference/http/) | an HTTP response |
| `Stream[T]` | — | a value-over-time source (see [Stream](#stream)) |
| `Query[T]` | — | a lazy read over `store` storage (see [Query](#query)) |
| `Set[T]` | — | an immutable set of value-keyable elements (see [Set](#set)) |

`ValidationError` is the error type returned by refined-type `.of` constructors.

## Stream

`Stream[T]` (v0.100) is a **lazy, pull-shaped sequence of values produced over
time** — the primitive for incremental output, distinct from `Effect[T]` (which
resolves exactly once) and `Query[T]` (a snapshot read over storage). Like those
neighbours it is **non-serialisable, non-storable, non-boundary, and not
value-comparable**: a live source is built and consumed in place, never persisted,
sent across a context boundary, or compared with `==`.

Within a Worker a stream moves freely (#2623):

- **Through a capability.** An operation may take or return a stream
  (`fn read(key: String) -> Effect[Download]`), since a capability's provider runs
  in the same isolate as its caller, so nothing is serialised.
- **Inside a record or sum.** A field may be a stream
  (`type Download = { contentType: String, body: Stream[Bytes] }`). The type is
  then *stream-carrying*: non-boundary like the stream, with no JSON codec, and
  refused wherever a stream is (a handler's parameter or result, a store field, an
  event) with `bynk.types.stream_at_boundary`, whose note names the field the
  stream is reached through.

**A stream is read once.** Draining it with `collect`, or handing it to a
response, consumes it, and reading it again faults with
`bynk: a stream can be read only once`. A builder (`map`, `take`) consumes its
source when it is first read, not when it is built. A stream from the platform
can only be read once, so an in-memory one behaves the same.

The v1 vocabulary is deliberately minimal:

| Form | Type | Purpose |
|---|---|---|
| `Stream.of(xs)` | `List[T] -> Stream[T]` | build a stream from a list (the deterministic source) |
| `s.map(f)` | `(T -> U) -> Stream[U]` | lazily transform each element |
| `s.take(n)` | `Int -> Stream[T]` | bound the stream to the first `n` elements |
| `s.collect()` | `Effect[List[T]]` | drain the stream to a list (the terminal) |

Errors ride **in-band** as `Result` elements (`Stream[Result[T, E]]`); a fault in
the producer aborts the stream as faults abort handlers.

A stream's end-to-end uses are [**streamed HTTP responses**](/book/reference/http/#streamed-responses):
`Streaming(stream)` returns an SSE body consuming a `Stream[String]`, and
[`RawStreamed(body, contentType)`](/book/reference/http/#streamed-raw-responses)
a raw body from a `Stream[Bytes]`. A richer
combinator vocabulary and live runtime sources are later increments of the
real-time track.

## Query

`Query[T]` (v0.92; ADRs 0115/0119) is a **lazy read over a `store`'s storage** —
the lazy receiver of the same combinator vocabulary the eager [`List`
methods](#list-methods) carry, dispatched by **receiver provenance**: a chain
rooted in a `store reservations: Map[K, V]` field is a `Query`, while the same
names on an in-memory `List` are eager. Like `Effect`/`Fn`/`Stream` it is
**non-storable and non-boundary** — rejected in any storable or boundary position
(`bynk.types.query_at_boundary`) — but is otherwise first-class: nameable,
returnable from a pure helper, passable. A query is **agent-local** and reads
**staged** state (read-your-writes).

**Builders** are pure and return a further `Query` — `filter`, `map`, `flatMap`,
`sortBy`, `take`, `skip`, `distinct`, plus the joins and `groupBy` below.

**Terminals** execute the query and are `Effect`-typed (awaited with `<-`), folding
into the storage capability the `store` fields already carry (no new `given`):

| Terminal | Result |
|---|---|
| `.collect()` | `Effect[List[T]]` |
| `.first()` | `Effect[Option[T]]` |
| `.count()` | `Effect[Int]` |
| `.sum(key)` / `.min(key)` / `.max(key)` / `.average(key)` | `Effect[…]` (empty-total: `Option`, or the zero for `sum`) |
| `.any(p)` / `.all(p)` | `Effect[Bool]` |
| `.fold(init, f)` | `Effect[acc]` |
| `.forEach(f)` | `Effect[()]` |

### Joins and grouping

Joins and grouping (v0.92+; ADR 0120) take an **`into:` combiner** that projects
each result through a lambda into a **user-named type** — bynk has no anonymous
pair/tuple, so a join row is always a named record. The arguments are positional
(`left:`/`right:`/`into:` name them for readability):

| Form | Yields |
|---|---|
| `joinOn(other, left: T -> K, right: U -> K, into: (T, U) -> V)` | equi-join → `…[V]` |
| `leftJoin(other, left: T -> K, right: U -> K, into: (T, Option[U]) -> V)` | left outer → `…[V]` |
| `join(other, on: (T, U) -> Bool, into: (T, U) -> V)` | predicate (nested-loop) → `…[V]` |
| `groupBy(key: T -> K, into: (K, List[T]) -> V)` | grouping → `…[V]` |

Each yields a `Query[V]` over storage and a `List[V]` eagerly. Because every result
is a named `V`, chained joins stay flat and named — no nested pairs. An equi-`joinOn`
whose probed key is [`@indexed`](/book/reference/agents/) routes through the index.

### Map key accessors

A `store Map[K, V]` roots three key-aware queries (v0.158; ADR 0184):

| Accessor | Yields |
|---|---|
| `map.keys` | `Query[K]` |
| `map.values` | `Query[V]` |
| `map.entries` | `Query[MapEntry[K, V]]` |

`.entries` exposes each entry as a **`MapEntry[K, V]`** — a compiler-known nominal
record `{ key: K, value: V }`, read with `.key`/`.value`. bynk has no tuple/pair
(ADR 0120), so an entry is a *named* record; the whole single-argument vocabulary
above applies to `.entries` unchanged. `MapEntry` is
[non-boundary](#query) (it is a generic-record instantiation, ADR 0183), so a read
handler projects each entry into a named type before its terminal:

```bynk,fragment
// the id lives in the key; project it back into the boundary shape
items.entries.map((e) => TodoItem { id: e.key, seq: e.value.seq, title: e.value.title, done: e.value.done }).collect()
```

This is what lets a stored value drop the denormalised copy of its own key. The
accessors are paren-less builders on the *storage* map, distinct from the eager
in-memory `Map` value methods `.keys()`/`.values()` that return `List`s.

## List methods

`List[T]` (v0.88; ADR 0116) carries the query algebra's **eager, in-memory**
combinator vocabulary as kernel methods, so a chain reads
`xs.filter((x) => x > 2).map((x) => x * 2)` (the same names the lazy
[`Query`](#query) carries over storage; the receiver decides eager vs lazy).

**Builders** (return a `List`): `map`, `filter`, `flatMap`, `sortBy`, `take`,
`skip`, `distinct`, `distinctBy`.

**Adding items** (return a new `List`; the receiver is unchanged): `prepend(x)`
puts `x` at the front, `append(x)` puts it at the end, and `concat(other)`
joins `other` after the receiver. A fold that builds a list in order uses
`append` (or `concat`), with no `reverse` afterwards:

```bynk
fn labels(ns: List[Int]) -> List[String] {
  let none: List[String] = []
  ns.fold(none, (acc, n) => acc.append(n.toString()))
}

fn withTotal(ns: List[Int]) -> List[Int] {
  ns.concat([ns.fold(0, (acc, n) => acc + n)])
}
```

`labels([1, 2, 3])` is `["1", "2", "3"]`, and `withTotal([1, 2])` is
`[1, 2, 3]`. `foldEff` builds in order the same way, its step returning
`Effect.pure(acc.append(x))`.

**Terminals**: `count`, `any`, `all`, `first`, `firstOrElse`, `sum`, `min`, `max`,
`average`.

Ordering keys (`sortBy`/`min`/`max`) come from the closed orderable base set —
`Int`/`Float`/`String`/`Duration`/`Instant`, refined types widening, opaque keys
rejected (`bynk.types.key_not_orderable`). Numeric keys (`sum`/`average`) are
`Int`/`Float`/`Duration` (`bynk.query.sum_needs_numeric`), with `average -> Float`.
**Empty aggregates are total** — `first`/`min`/`max`/`average` return `Option`,
`sum` the zero. The first-party `bynk.list` free functions are the deprecated
predecessors of these methods (see [Operators & built-ins](/book/reference/operators/) and
[First-party `bynk` capabilities](/book/reference/bynk-capabilities/)).

## Set

`Set[T]` is a built-in immutable set, beside `List` and `Map`. It keeps
**insertion order**, as `Map` does: `toList()` and the wire form list the
elements in the order they were first added, and inserting an element that is
already there keeps its place.

```bynk,fragment
let tags = Set.of(["b", "a", "b"])        // {b, a}
let more = tags.insert("c").remove("a")   // {b, c}; `tags` is unchanged
let none: Set[Int] = Set.empty()
```

- **Members** follow the `Map`-key rule: `Int` or `String`, or a refined or
  opaque type over them. Anything else is `bynk.types.unkeyable_set_element`,
  whether written (`Set[Point]`) or inferred (`Set.of([p])`).
- **Statics:** `Set.empty()` takes its element type from context, as
  `List.empty()` does, and `Set.of(xs: List[T])` drops repeats, keeping each
  element's first position.
- **Kernel:** `length()`, `contains(x)`, `insert(x)`, `remove(x)` and
  `toList()`. `insert` and `remove` return a new set.
- **Combinators** are the `bynk.set` commons (`uses bynk.set`): `union`,
  `intersect`, `difference`, `isSubsetOf`, `filterSet` and `mapSet`.
- **Equality** is membership: `Set.of([1, 2]) == Set.of([2, 1])`.
- **On the wire** a set is a JSON array in insertion order. Decoding one with a
  repeated element fails, since a Bynk sender never writes one.

A `store members: Set[T]` field is a different thing: the storage set of an
agent, with effectful `add`/`remove`/`contains`/`size`. A `Set[T]` value is
the immutable collection described here.

## Connection

`Connection[F]` (v0.102) is a **held resource** — a typed handle to a long-lived
WebSocket connection, where `F` is the type of frames the server can send. It is
the one concrete instance of the closed **`Held`** kind. Held values are
**runtime-produced** (there is no constructor — they arrive from a capability
operation or a handler parameter the framework supplies) and governed by an
**ownership discipline** (the *linearity* rules, §2.9): a held value has at most
one owner, and must be **disposed** — stored, closed, or transferred — before its
scope exits.

| Operation | Type | Notes |
|---|---|---|
| `c.send(f)` | `F -> Effect[()]` | write a frame; **non-consuming** (the binding stays owned) |
| `c.close()` | `Effect[()]` | end the connection; **consuming** (the binding is spent) |

Held values are **non-serialisable, non-boundary, and not value-comparable** —
they may not cross a context boundary, be compared with `==`, or be stored except
in `Cell[Option[Connection]]` / `Map[K, Connection]` (a `Set`/`Log`/`Cache`
rejects them). Storing one (`conns.put(u, c)`) or closing it (`c.close()`) disposes
it; using it afterward, or letting it escape a handler undisposed, is a compile
error. The compiler reports an undisposed connection (`bynk.held.leak`), a use after
disposal (`bynk.held.use_after_consume`), and branches that dispose inconsistently
(`bynk.held.branch_divergence`).

### WebSocket services

> The full protocol surface — the `on open` / `on message` / `on close` handlers,
> edge authentication, broadcast over a held `Map`, the `TestConnection` model, and
> the platform mapping — is on the [WebSocket reference page](/book/reference/websocket/); the
> worked chat-room is the guide [Handle a WebSocket connection](/book/guides/entry-points/websocket/).
> This section summarises how a connection is produced.

A `service … from websocket(in:, out:)` produces connections. The upgrade
**authenticates at the edge** — like an HTTP route, `on open` must name its actor
with `by` (there is no anonymous upgrade; a browser `WebSocket` carries a Bearer
token in the `Sec-WebSocket-Protocol` subprotocol, since it cannot set an
`Authorization` header) — and the handler receives a fresh, owned `Connection[out]`
it must dispose, the canonical disposal being transfer into an agent:

```bynk,fragment
service ChatGateway from websocket(in: ClientFrame, out: ServerFrame) {
  on open (roomId: RoomId) -> Effect[()] by user: Participant {
    let _ <- connection.send(ServerFrame { text: "welcome" })
    let _ <- Room(roomId).join(user.identity, connection)
    ()
  }
}
```

The service holds **exactly one** `on open`; inbound frames then arrive at the
agent that owns the connection through the explicit `on message` / `on close`
handlers, and the agent fans frames out to many connections by holding them in a
`Map` and broadcasting over it. On the **bundle** target the connection is a
`TestConnection` — a capture-and-inspect channel that records every frame sent — so
a WebSocket service is fully developable and testable with no Durable Object. On
the **Workers** target the connection maps onto a Durable Object using the
hibernatable-WebSocket API: a `Connection` stored in agent state survives
hibernation and is restored on rehydration.

## The JSON codec

Three compiler-backed statics decode and encode JSON at a typed boundary:

| Form | Type | Purpose |
|---|---|---|
| `Json.encode(v)` | `String` | serialise a checked value to a JSON string |
| `Json.decode[T](s)` | `Result[T, JsonError]` | parse a JSON string into `T`, validating structure (and any refinements) |
| `Json.decodeSql[T](s)` | `Result[T, JsonError]` | `decode` for a `D1` row or list of rows, which also reads `0`/`1` as a `Bool` ([SQL databases](/book/reference/bynk-capabilities/#sql-databases-d1)) |

`Json.decode[T]` takes an explicit type argument and validates the decoded value
against `T` — including refined-type predicates — so untrusted JSON enters the
program only as a fully-checked value. `JsonError` is the error it returns
(malformed JSON, or a structural/refinement mismatch). See the guide
[Decode untrusted JSON into a typed value](/book/guides/type-system/decode-json/).

`Json.encode` writes an `Option[U]` in Bynk's tagged form:
`{"kind": "Some", "value": …}` or `{"kind": "None"}`. `Json.decode` accepts
that form and also the forms other JSON producers use for an optional value:

| JSON for an `Option[U]` | Decodes to |
|---|---|
| the key is absent from its record | `None` |
| `null` | `None` |
| `{"kind": "None"}`, with no other keys | `None` |
| `{"kind": "Some", "value": v}`, with no other keys | `Some(v)`, `v` checked as `U` |
| any other value `v` | `Some(v)`, `v` checked as `U` |

The tagged forms are tried first, so whatever `Json.encode` writes decodes back
to the same value, even for an `Option[Option[T]]`. The same decoder runs at
every boundary, so an HTTP request body, a cross-context call and a stored agent
field accept these forms too.

The tagged forms are recognised only when the object has exactly those keys.
Every payload Bynk wrote decodes as before, but a hand-written tagged object
with an extra key, such as `{"kind": "Some", "value": "d", "note": "x"}`, is
read as a bare value, so it fails for an `Option[String]`.

## Type aliases

```bynk
type Id = Int
```

An alias introduces a distinct named type. Even a plain alias is branded in the
emitted TypeScript and carries a `.of` constructor (like any refined type, it has
no `.unsafe` — that is opaque-only; ADR 0182).

## Record types

A record groups named, immutable fields:

```bynk
type Order = {
  id: String,
  item: String,
}
```

- **Construct** by naming every field: `Order { id: "1", item: "book" }`.
- **Access** with dot notation: `o.id`.
- **Update** with the spread form, which copies and overrides:
  `Order { ...o, item: "pen" }`.

Records emit a TypeScript `interface` with `readonly` fields. A record field may
not directly be of the record's own type (`bynk.resolve.recursive_record_field`).

### Documenting fields and variants

A record field, a sum variant, a variant's payload field and a `messages` entry
each take a doc on the line above them, as a `--- text` line or a `---` block.
Hover, completion and the documentation page show it. See
[Document fields and variants](/book/guides/program-structure/document-your-code/#document-fields-and-variants).

## Generic record types

A record type may take **type parameters** — a name scoped to the declaration,
optionally bounded (see [Generics and bounds](/book/reference/generics/)) — so one
shape serves many element types. The common
case is an API envelope:

```bynk
type Paginated[T] = {
  items: List[T],
  cursor: Option[String],
}

type User = { id: Int, name: String }

fn first_page(users: List[User]) -> Paginated[User] {
  Paginated { items: users, cursor: None }
}

fn items_of(page: Paginated[User]) -> List[User] {
  page.items
}
```

- **Apply** the type to concrete arguments wherever a type is expected:
  `Paginated[User]`, `Keyed[String, Int]`.
- **Construct** as an ordinary record; the type arguments are **inferred** from
  the field values. When a field cannot pin them down (an empty `items` list),
  annotate the binding: `let p: Paginated[User] = Paginated { items: [], cursor:
  None }`.
- **Access** fields with the argument substituted in: `page.items` has type
  `List[User]`.

Only a record body may be generic — a refined, opaque, or sum type cannot take
type parameters (`bynk.generics.generic_non_record`). A generic type must be
applied to exactly its declared number of arguments
(`bynk.generics.type_arg_count`).

Generic records emit an **erased** TypeScript generic interface — `type
Paginated[T]` becomes `export interface Paginated<T>` — exactly as a generic
function erases to `function f<A>(…)`.

### Generic records at the boundary (v0.174)

A generic-record instantiation is **serialisable** — it may appear in a field of
another record, a sum payload, a service or agent handler signature, agent
state, or a `Json.encode`/`Json.decode` target — exactly when its type arguments
are. The compiler generates a **monomorphised codec** per instantiation: a
`Paginated[User]` boundary emits `__serialise_Paginated_User` /
`__deserialise_Paginated_User`, specialised to the concrete arguments and
delegating to their codecs (`__serialise_List_User`, `__serialise_Option_String`).
The emitted TypeScript interface stays the erased `Paginated<T>`; only the codec
is per-instantiation, matching how `List`/`Map`/`Result` already specialise.

```bynk,fragment
fn save(page: Paginated[User]) -> String {
  Json.encode(page)                          // __serialise_Paginated_User
}

fn load(s: String) -> Result[Paginated[User], JsonError] {
  Json.decode[Paginated[User]](s)            // __deserialise_Paginated_User
}
```

A **non-serialisable argument** is still rejected, at the argument: a function,
`Query`, `Stream`, or `Connection` type inside the instantiation
(`Paginated[Int -> Int]`) draws that type's own boundary error
(`bynk.types.function_at_boundary`, …), and an uncodable `Json` target draws
`bynk.types.json_uncodable`.

A **recursive** generic record — one that transitively contains itself, whether
directly, through another record, or through an `Option`/`List` wrapper
(`type Node[T] = { value: T, next: Option[Node[T]] }`) — has no finite set of
monomorphised codecs, so it is rejected with
`bynk.generics.recursive_generic_at_boundary`. Because a record field is itself a
boundary position, the self-referential field makes such a type **undeclarable**,
not merely unusable at a boundary — the error fires at the declaration. (A
*non-generic* recursive record is unaffected: its single codec is
self-referential and terminates on the data, so it declares and serialises as
before; only the per-instantiation generic form is deferred.) Use a concrete
recursive type, or break the cycle.

A generic type may carry **instance methods**. A method whose receiver is a
generic type sees the type's own parameters in scope alongside any it declares
itself, so `fn Box.map[U](self, f: A -> U) -> Box[U]` reads `A` from the receiver
`Box[A]` and infers `U` from the argument. It erases to a generic
namespace-object method — `map<A, U>(self: Box<A>, f: (a: A) => U): Box<U>` — the
type's parameters threaded onto each method (the namespace object itself cannot
carry them). The type arguments are inferred from the receiver and the argument
types; the explicit `x.map[U](…)` form is a later increment. **Static** methods
on a generic type stay deferred (`bynk.generics.method_on_generic_type`) — they
have no receiver to supply the type's parameters. A refined or opaque body cannot
be generic (its base is a fixed primitive); only a record or **sum** body may be
(`bynk.generics.generic_non_record`), and generic sums are described under
[Sum types](#sum-types).

## Sum types

A sum type is one of several variants; a variant may carry a payload. The
**pipe form** is required whenever any variant carries a payload:

```bynk
type Status =
  | Pending
  | Shipped(tracking: String)
  | Cancelled(reason: String)
```

When **every** variant is payloadless, write the **`enum` form** — it is the
canonical spelling for a payloadless sum, and these docs use it throughout:

```bynk
type Suit = enum { Hearts, Diamonds, Clubs, Spades }
```

`enum { A, B, C }` is exactly sugar for `| A | B | C`; the two are the same type.
Reach for the pipe form only when a variant needs a payload.

- **Construct** by naming a variant: `Pending`, `Shipped("1Z…")`.
- **Consume** with [`match`](#matching) or [`is`](/book/reference/operators/).
- **Document** a variant, or a payload field, with a doc on the lines above it,
  as a [record field](#documenting-fields-and-variants) is.

Sum types emit a discriminated union keyed on a `tag` field. As JSON, a variant
is a flat object whose `kind` key names the variant (`{"kind": "Shipped",
"tracking": "1Z…"}`), so a payload field cannot be named `kind`
(`bynk.resolve.reserved_payload_field`). A payload field named `tag` is fine:
it keeps that name on the wire.

### Error embeddings (v0.154)

A sum used as an error type may declare **embeddings** — a trailing
`embeds <type> as <Variant>` clause naming a single-payload variant that wraps a
sub-error. The [`?` operator](/book/reference/operators/) then converts that
sub-error automatically, replacing the boilerplate `.mapErr(Wrap)` on every
cross-context chain:

```bynk,fragment
type OrderError =
  | OutOfStock(sku: Sku)
  | Payment(reason: PaymentError)
  | Fulfilment(reason: ScheduleError)
  embeds PaymentError as Payment, ScheduleError as Fulfilment
```

Each `embeds E as V` requires `V` to be a variant of the same sum with exactly
one payload field of type `E`, and a given `E` may be embedded by only one
variant (so the conversion is unambiguous). The conversion is **one level**: `?`
converts a `Result[T, E]` into `Result[_, F]` only when `F` declares `E`
directly. Mapping a domain error to an *HTTP* status stays an explicit `match`
in the handler — by design, not an embedding.

### Generic sums

A sum body may carry `[A, B]` type parameters, exactly as a record body does. A
parameter is an unconstrained, bound-free name resolved as a rigid variable
inside the variant payloads:

```bynk
type ApiResult[T] =
  | Loaded(value: T)
  | Failed(message: String)
```

A reference applies concrete arguments (`ApiResult[User]`). **Construction**
infers the arguments argument-directed from the variant's payload
(`Loaded(user)` is `ApiResult[User]` because `user : User`); a variant whose
payload cannot determine a parameter — a payload-less variant, or one that does
not mention it (`Failed`) — leans on the binding's expected type as the pressure
valve (`let r: ApiResult[User] = Failed("...")`), and reports
`bynk.generics.uninferable_type_arg` when neither grounds it. **`match`**
substitutes the arguments into each arm's payload binding (`Loaded(v)` binds `v :
User`). Emission is **erased** TypeScript generics — one
`export type ApiResult<T>` discriminated union, with each payload constructor a
generic arrow (`Loaded: <T>(value: T): ApiResult<T> => …`).

Like a generic record, a generic-sum instantiation is **serialisable** exactly
when its type arguments are, through a monomorphised codec per instantiation
(`__serialise_ApiResult_User` / `__deserialise_ApiResult_User`); a recursive generic
sum has no finite codec set and is rejected at a boundary with
`bynk.generics.recursive_generic_at_boundary`. A generic sum may not carry an
`embeds` clause (`bynk.generics.generic_sum_embeds`).

## Opaque types

An opaque type is backed by another type but is nominally distinct:

```bynk
type OrderId = opaque String
```

- Construct only via `OrderId.of(...)` (checked, returns `Result`) or
  `OrderId.unsafe(...)` (unchecked); record syntax is rejected
  (`bynk.resolve.opaque_record_construction`).
- Construction and inspection are confined to the defining module/context.
- Opaque types are **excluded** from [literal admission](/book/reference/refined-types/).

## Refined types

A base type plus a predicate. See the [refined-type reference](/book/reference/refined-types/).

## Regular expressions

A refined `String where Matches("…")` checks a whole string against a pattern.
Six `String` methods use the same patterns to look inside one:

```bynk,fragment
let ok = sku.matchesRegex("[A-Z]{3}-[0-9]{4}")         // the whole string
let has = text.containsRegex("(?i)error")               // anywhere, ignoring case
let v = text.findRegex("v(\\d+)\\.(\\d+)")            // Some([Some("v2.14"), Some("2"), Some("14")])
let all = text.findAllRegex("\\d+")                     // every match
let tidy = text.replaceRegex("\\s+", " ")              // every match; `$1`, `$<name>`, `$$`
let parts = csv.splitRegex("\\s*,\\s*")              // captured groups are not kept
```

- **The pattern must be a string literal.** It is checked when the program
  compiles, as a `Matches` pattern is: its syntax, and that it cannot take
  catastrophic time on a crafted input (ReDoS). A pattern built at run time
  would escape that check, so it is refused (`bynk.types.regex_not_literal`).
- **Captures** come back as a list: element 0 is the whole match, then each
  group, `None` for a group that did not take part.
- **Flags** go at the start of the pattern: `(?i)` ignores case, `(?m)` lets
  `^`/`$` match at line breaks, `(?s)` lets `.` match a newline.
- **Semantics** are JavaScript's `RegExp`, without the `u` flag, as `Matches`
  uses.
- **`replaceRegex`** reads `$1`, `$<name>`, `$&` and `$$` in the replacement,
  unlike `replace`, which inserts its text literally.

## Matching

`match` branches on every variant of a sum/`Result`/`Option`, binding payloads:

```bynk,fragment
match s {
  Pending => "…"
  Shipped(tracking: t) => t
  Cancelled(reason: r) => r
}
```

A `match` must be exhaustive (`bynk.types.non_exhaustive_match`); a `match` is an
expression whose arms must join to a common type — their least upper bound, so a
refined type and its base agree at the base (`bynk.types.match_arm_mismatch`).

Every arm must also be reachable (`bynk.types.unreachable_arm`): an arm is an
error when each value its pattern matches is already matched by an earlier
arm. Each of these is rejected at the marked arm:

```bynk,fragment
match o {
  Some(_) => 1
  Some(Red) => 2   // `Some(_)` already matches every `Some`
  None => 3
}
```

```bynk,fragment
match c {
  Red => 1
  Green => 2
  Blue => 3
  _ => 4           // the variants above are already exhaustive
}
```

```bynk,fragment
match o {
  Some(Red | Green) => 1
  Some(Blue) => 2
  Some(Green) => 3 // the first arm's or-pattern already matches it
  None => 4
}
```

A narrower arm may precede a wider one (`Some(Red)` then `Some(_)`), and a
guarded arm never counts as covering, because its guard may fail. So an
unguarded arm with the same pattern can always follow it.

### Record patterns

A record is matched by its fields with a record pattern, `TypeName { f1, f2: pattern, .. }`
(patterns track, slice 1, #2641):

```bynk,fragment
match order {
  Order { status: Paid, total, .. } if total > 100 => "large paid order"
  Order { status: Paid, .. } => "paid"
  _ => "other"
}
```

```bynk,fragment
if o is (Order { status: Cancelled, .. }) { "cancelled" } else { "live" }
```

- The type name is required. A bare field `total` binds a local `total`;
  `status: Paid` matches the field against a sub-pattern, which can be any
  pattern, a nested record pattern included. Record patterns nest anywhere a
  pattern can, as in `Some(Order { total, .. })`.
- The trailing `..` is **required**
  (`bynk.parse.record_pattern_without_rest`). A record pattern is always open:
  the fields it does not list are ignored, so **adding a field to a record never
  breaks a pattern**. The editor offers a one-click fix that inserts `, ..`.
- After `is`, a record pattern is **parenthesised**
  (`bynk.parse.record_pattern_in_is_unparenthesised`), because there the `{`
  would also open the `if`'s block. The editor offers a fix that adds the
  parentheses.
- The type must be a record (`bynk.types.record_pattern_on_non_record`), every
  listed field must exist (`bynk.types.unknown_field`) and appear once
  (`bynk.types.duplicate_pattern_field`). A generic record's type arguments are
  taken from the matched value and are never written.
- A record pattern reads fields as `.field` does, so it is refused on another
  context's `exports opaque` type (`bynk.context.opaque_inspection`).
- **Exhaustiveness is conservative.** A record pattern covers its type only when
  every field is matched by a name or `_`; one with a literal or variant field
  is partial, so end the `match` with a `_` arm. Precise coverage across several
  arms is a later increment.

