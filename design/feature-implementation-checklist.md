# Bynk — Design Notes vs. Implementation Checklist

_Comparing `design/bynk-design-notes.md` (working draft, 9 May 2026) against the
compiler as it stands at **~v0.107.0** (head `cd8aa77`, latest ADR **0135**)._

**Legend:** ✅ implemented (parsed + checked + emitted) · 🟡 partial · ❌ not yet
implemented · 🚫 resolved as "won't build" by an ADR.
Most ❌ items are **deliberately deferred** to the remaining v1 coordination
surface, not oversights.

> **Update note (this revision):** since the previous revision (v0.97) a whole
> **real-time / WebSocket track** has landed (ADRs 0128–0135): the `Stream[T]`
> value-over-time primitive, streaming HTTP (SSE) responses, held-resource
> linearity with `Connection[F]`, the `from WebSocket` protocol (edge-auth
> `on open`, inbound `on message`/`on close`, hibernation re-association), and
> **`parTraverse`** (parallel broadcast). HTTP status moved to the full RFC 9110
> vocabulary (ADR 0126), and agents now own their capabilities with
> requirement-provenance surfaced (ADR 0127). What's still outstanding is
> essentially **events, sagas/idempotency, alarms, and a few primitives**.
>
> _Earlier (v0.80 → v0.97): the **storage-kind catalogue** (ADRs 0108–0113, 0121,
> 0124, 0125) and the **query algebra** (ADRs 0114–0120) landed; `state{}` /
> `commit` were removed in favour of `store` (ADR 0123); `Duration` / `Instant`
> shipped (ADRs 0112/0114)._

---

## Declarations & top-level kinds

| Feature (design §) | Status | Notes |
|---|---|---|
| `type` — records, sum/ADT types | ✅ | pipe and `enum` sum forms; nominal records |
| `actor` contracts (§6) | ✅ | `by` clause, context-sealed verified identity |
| — auth: Bearer (JWT/HS256), Signature (HMAC), None, Internal | ✅ | ADRs 0085/0089 |
| — auth: mTLS | ❌ | explicitly out of v0 scope |
| `service` + protocols (§7) | ✅ | `from <protocol>`; `call`-only default |
| — `from HTTP` (methods, path params, typed body, `HttpResult`) | ✅ | RFC 9110 status vocabulary (2xx/3xx redirects/4xx/5xx); ADR 0126 |
| — `from Queue` (consumer, Ack/Retry) and `Cron` | ✅ | ADR 0002/0078 |
| — `from Events` (subscription) | ❌ | deferred — no Events track yet |
| — `from WebSocket` protocol | ✅ | edge-auth `on open`, held-connection transfer, hibernation; ADRs 0131–0135 |
| — Alarm protocol | ❌ | deferred (no `Alarms` capability yet) |
| — streaming HTTP response (SSE) | ✅ | `Streaming` (200) variant carrying `Stream[String]`, ADR 0129 |
| `agent` — state + identity | ✅ | → Durable-Object-style classes |
| — agent state surface | ✅ | now via `store` fields only; `state{}`/`commit`/`self.state` removed (ADR 0123) |
| `fn` — module-level pure + agent-level `given` | ✅ | generics, lambdas, closures; agents own their capabilities (ADR 0127) |
| `on` handler clauses | ✅ | Call / HTTP / Queue / Cron + WebSocket `open`/`message`/`close` (not alarm/event) |
| `store` storage fields | ✅ | full storage-kind catalogue (see below); atomic commit at handler boundary (ADR 0109) |
| `event` declarations + `Events.emit` + `EventEnvelope` | ❌ | no `event` keyword in lexer |
| `context` / `commons` / `test` contexts | ✅ | all three top-level kinds |
| Visibility: opaque / transparent / private; `uses` / `consumes` / `exports` | ✅ | enforced in resolver |
| `provides` (capability substitution / providers) | ✅ | constructor-injection, cycles rejected |
| adapters (logic-free unit kind) | ✅ | ADR 0010 |

## Storage types (§10)

| Feature | Status | Notes |
|---|---|---|
| `Cell[T]` — single value, implicit deref | ✅ | ADR 0109 |
| — write forms `:=` (unconditional) vs `.update(fn)` (read-modify-write) | ✅ | `.update` landed in ADR 0125 (newest commit) |
| `Map[K,V]` storage kind — effectful entry ops (put/update/upsert/remove/get) | ✅ | ADR 0110 (distinct from immutable value-`Map`) |
| `Set[T]` — effectful membership ops | ✅ | ADR 0110 |
| `Cache[K,V]` — TTL expiry, time via `given Clock` | ✅ | ADR 0113 |
| `Log[T]` — append-only, time-indexed | ✅ | ADR 0121 |
| Storage annotations `@indexed` / `@ttl` / `@retain` / `@bounded` | ✅ | closed `@name(args)` registry, ADR 0111 |
| Rehydration validation (refined fields revalidated on load) | ✅ | `RehydrationViolation` fault, ADR 0124 |
| Immutable `List[T]` / `Map[K,V]` collection values | ✅ | distinct from storage kinds |
| `Kv` durable binding storage | ✅ | ADRs 0050/0051 |
| `Ref[A]` agent handle | 🟡 | cross-context/agent calls work, but `Ref` is not a first-class storage kind |
| `Queue[T]` storage kind | 🚫 | ruled out — Queue is a delivery concern, not storage (ADR 0122) |
| `Connection[F]` / `Held[T]` (held resources) | ✅ | linearity-checked, non-serialisable, storable only in `Cell`/`Map`, disposed before scope exit; ADR 0130 |

## Query algebra (§11)

| Feature | Status | Notes |
|---|---|---|
| Lazy `Query[T]` over storage `Map` | ✅ | v0.92, ADRs 0115/0119 |
| Eager `List` combinator vocabulary | ✅ | v0.88, ADR 0116 (bynk.list free fns deprecated → methods) |
| Builders: filter/map/flatMap/sortBy/take/skip/distinct/distinctBy | ✅ | checker kernels |
| Joins: `joinOn` / `leftJoin` / `join` (predicate) | ✅ | ADR 0120 |
| Grouping: `groupBy` | ✅ | ADR 0120 |
| Terminals: collect/first/firstOrElse/count/fold/sum/any/all/forEach | ✅ | checker kernels |
| Log time-window builders: since/before/between/recent/reversed | ✅ | ADR 0121 (checker + emitter) |
| Indexing — `@indexed` secondary indexes, routing, hygiene warnings | ✅ | slice 3, ADR 0118 + warning channel ADR 0117 |
| `traverse` (sequential effectful iteration) | ✅ | in stdlib |
| `parTraverse` (concurrent fan-out / broadcast) | ✅ | ADR 0135 (held-aware iteration borrow surface) |
| `traverseAll` / `parTraverseAll` (collect-all variants) | ❌ | not yet — only short-circuit forms ship |

## Capabilities, effects & failure model (§5, §12, §13)

| Feature | Status | Notes |
|---|---|---|
| `given` capability injection, `Effect[T]`, `<-` await | ✅ | + `Effect.pure`, tail auto-lift |
| `Result[T,E]`, outcomes vs faults, `?` propagation | ✅ | exhaustive `match`, `is` narrowing |
| Built-in capabilities: Clock, Random, Fetch/Http, Logger, Secrets, Config/IO | ✅ | first-party `bynk.bynk` adapter; agents own their capabilities, requirement provenance (ADR 0127) |
| Fire-and-forget send `~>` | ✅ | ADR 0106 |
| `Alarms` capability | ❌ | still deferred (the WebSocket/held-resources track shipped without it) |
| `Idempotency` capability (`dedup`) | ❌ | deferred |
| `Sagas` capability + LIFO compensation | ❌ | deferred |
| `attempt` / `recover` (fault → outcome) | ❌ | deferred |

## Refined types (§15)

| Feature | Status | Notes |
|---|---|---|
| Predicates: `Matches`, `InRange`, `MinLength`/`MaxLength`/`Length`, `NonNegative`, `Positive`, `NonEmpty` | ✅ | full vocabulary |
| Boundary validation (HTTP body, params, etc. before handler runs) | ✅ | constructor returns `Result[T, ValidationError]` |
| Refinement in storage / rehydration validation | ✅ | now wired through the storage kinds (ADR 0124) |
| External schema generation (OpenAPI/JSON-Schema from refined types) | ❌ | design aspiration; not built |

## Type system & primitives (§15)

| Feature | Status | Notes |
|---|---|---|
| HM inference, closed sums, nominal records, opaque types, parametric generics | ✅ | |
| No subtyping / no effect inference (by design) | ✅ | capabilities declared, not inferred |
| Deliberate exclusions: subtyping, HKT, row poly, type classes | ✅ (excluded) | intentionally out of scope |
| Base types `Int`, `String`, `Bool`, `Float` | ✅ | `Float` per ADR 0040 |
| `Duration` primitive (literal + arithmetic) | ✅ | ADR 0112 |
| `Instant` primitive (absolute time) | ✅ | ADR 0114 (the type-system spec's name since the spec adopted the built names) |
| `Stream[T]` value-over-time primitive | ✅ | ADR 0128 — non-serialisable/non-storable like `Query`/`Effect`/`Fn`; `Stream.of`/`map`/`take`/`collect` |
| `Bytes` primitive | ✅ | ADR 0142 |
| Exact-decimal primitive | ✅ | `Decimal` (#2622): arbitrary precision, kept scale, no `/` (`div` states a scale and rounding mode), a JSON string on the wire; the spec's earlier `Decimal` was renamed to the built `Float` |

## Validation & invariants (§14)

| Feature | Status | Notes |
|---|---|---|
| `test` contexts, `Mock[T]`, `provides` mocking, `assert`, call capture | ✅ | + integration tests over simulated wire (ADR 0009) |
| Agent `invariant` (runtime-checked at commit) | ✅ | v0.80 / ADR 0107 |
| — static provable-violation pass | ❌ | deferred follow-on |
| — typed agent-fault channel (distinguishable `InvariantViolation`) | ❌ | currently a bare 500 |
| Property-based testing / scenarios | ❌ | intended as library-level, not language |

## Schema versioning (§7)

| Feature | Status | Notes |
|---|---|---|
| Field defaults on **agent state** (store fields) | ✅ | inline initialisers |
| Field defaults on value/event types | ❌ | tied to events |
| `@schema(N)` annotation, `via schema(…)` clause, `schemaVersion` envelope | ❌ | deferred with events |
| Additive-evolution merge on rehydration | 🟡 | storage rehydration merges additively (ADR 0124); full event-schema story still deferred |

## Syntax (§16)

| Feature | Status | Notes |
|---|---|---|
| Glyphs `->` `<-` `=>` `:=` `==`/`!=`/`<=`/`>=` `&&`/`\|\|`/`!` `?` `..` `~>` | ✅ | `:=` is now the storage-write surface (ADR 0109); `~>` send (ADR 0106) |
| String interpolation `\(expr)`, numeric literal separators/bases | ✅ | |
| `is` (pattern-as-Boolean), `implies` (in invariants) | ✅ | ADR 0007 / v0.80 |
| No pipe `\|>`, no custom operators (by design) | ✅ (excluded) | |

---

## Bottom line

The implemented surface keeps growing. On top of the storage-kind catalogue
(`Cell`/`Map`/`Set`/`Cache`/`Log`, `store` as the sole state surface, atomic commit,
annotations, rehydration validation), the query algebra (lazy `Query[T]`, eager `List`
vocabulary, builders, joins, `groupBy`, terminals, Log time-windows, `@indexed` routing),
and the `Duration`/`Instant` primitives, the compiler now ships the whole **real-time /
WebSocket track**: the `Stream[T]` primitive, streaming HTTP (SSE) responses, linearity-
checked **held `Connection[F]`** resources, the **`from WebSocket`** protocol (edge-auth
`on open`, inbound `on message`/`on close`, hibernation re-association), and **`parTraverse`**.

What remains **not yet implemented** — deferred by design — is now a short list:

1. **Events** — `event` decls, emission, pattern subscription, envelopes, schema
   versioning (`@schema` / `via schema`). *(Nothing in the lexer yet.)*
2. **Sagas / compensation**, the **`Idempotency` capability**, and `attempt`/`recover`.
   (A durable-provider path via Cloudflare Workflows' saga rollbacks looks viable.)
3. **`Alarms` capability** + the Alarm protocol (the held-resources track shipped WebSocket
   but not alarms).
4. **`traverseAll` / `parTraverseAll`** collect-all variants, **external schema
   generation**, and **mTLS**.
5. **Invariant follow-ons** — static provable-violation pass; typed agent-fault channel.

Resolved-by-decision rather than pending: **`Queue` as a storage kind** was ruled out
(it's a delivery concern, ADR 0122), and **`Ref[A]`** as a first-class storage handle
remains unbuilt while cross-context/agent calls already cover the need.

Genuine gaps within current scope (per the project's own audit): `Int` precision beyond
2^53 (emits JS `number`), and `any`-typed boundary emission in `workers` mode.
