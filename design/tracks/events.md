# The Events protocol — in-system pub-sub for cross-context decoupling

- **Status:** Slicing — slices 0–4 have shipped (§7); slice 4b, range
  patterns in `via schema(...)`, is open as [#990](https://github.com/accuser/bynk/issues/990). This began as a
  settling-draft doc per
  [ADR 0167](../decisions/0167-feature-tracks-run-github-native.md): the
  **spine issue** is
  [#936](https://github.com/accuser/bynk/issues/936); this doc lands via a
  draft PR referencing it (*"Part of #936"*, never `Closes`). §3.1, §3.2, §3.3,
  and §3.6 are now genuinely settled, with their foundational ADRs landed as
  [ADR 0284](../decisions/0284-events-fanout-substrate.md),
  [ADR 0285](../decisions/0285-events-protocol-set-extension.md),
  [ADR 0286](../decisions/0286-events-pattern-dispatch-deliver-and-filter.md),
  and [ADR 0287](../decisions/0287-events-replay-out-of-scope.md); §3.5 is a
  named slice-3 concern. **Slice 0 has shipped** (#939): `event` declarations,
  `given Events` emission with owner-only enforcement, and unpatterned `from
  Events(E)` subscription, across contexts and across all three platforms —
  its own implementation decisions (the concrete fan-out mechanism per
  platform, and the owner-only checker pass) landed as
  [ADR 0288](../decisions/0288-events-slice0-fanout-implementation.md) and
  [ADR 0289](../decisions/0289-events-owner-only-emission-check.md). **§3.4 is
  now also settled, empirically:** per-publisher FIFO holds within one
  emission batch and across successive non-overlapping calls to one agent,
  but not across concurrent invocations of the same agent — narrower than §7
  originally asserted, measured on real `workerd` and recorded in
  `design/pending/events-per-publisher-fifo-scope.md` (pre-stamp). **Slice 1
  has shipped** (#966): `from Events(E { field: value, .. })` subscription
  pattern filtering, via an Events-local pattern node rather than the shared
  `Pattern`/refined-pattern machinery §3.3 originally claimed — an amendment
  to ADR 0286's "no bespoke matching engine" wording, recorded in
  `design/pending/events-subscription-pattern-filtering.md` (pre-stamp).
  Delivery filtering only: the handler's parameter is **not** statically
  narrowed to the matched values. **Slice 2 has shipped** (#968): an
  `on event(e: E, env: EventEnvelope)` handler's optional second parameter
  carries `eventId`/`publisherId`/`emittedAt`/a reserved `schemaVersion`,
  minted once per emission so every subscriber it fans out to observes the
  same values, enabling the `Idempotency.dedup`/`remember` idiom keyed on
  `env.eventId`. `publisherId` amends §7's "the publisher is the emitting
  agent" claim to the emitting *context* instead (`Events.emit` is also
  legal from a plain, keyless service handler with no agent identity to
  report); §12's idempotency example is corrected against the capability's
  real, already-shipped API. Recorded in
  `design/pending/events-envelope-and-idempotency-idiom.md` (pre-stamp).
  Additive versioning (a real, computed `schemaVersion`) remains a later
  slice.
- **The one scope decision made up front.** This track delivers the **live
  pub-sub core** — declaration, emission, subscription with pattern refinement,
  the envelope, and additive versioning. It **splits event replay / backfill
  out** to a separate, future, currently-unfiled track (§3.6), which also
  absorbs the actors track's deferred **Q8 (replay/ordering)**
  ([#260](https://github.com/accuser/bynk/issues/260)). This mirrors
  [`idempotency-capability.md`](../archive/retired-tracks.md) §3.2, which shipped
  the in-memory provider and moved its durable half to a future track so its own
  scope "stays honest about what it actually delivers."
- **Realises:** `design/bynk-design-notes.md` §7 ("Events in depth", lines
  162–288) and the emit-lowering note (line 1321); the §12 event-subscriber
  idempotency idiom (lines 641, 656–666), whose substrate — the `Idempotency`
  capability keyed on `env.eventId` — **already shipped** ahead of this track
  ([#554](https://github.com/accuser/bynk/issues/554), the
  [`idempotency-capability.md`](../archive/retired-tracks.md) track).
- **Posture:** Feature track per
  [ADR 0076](../decisions/0076-feature-track-posture.md). Qualifies on all three
  axes (§2), including the **correctness/safety-boundary** one: cross-context
  fan-out where a wrong pattern-match delivers a fact to a subscriber that should
  not see it, and a wrong ordering or duplication story silently corrupts
  downstream state.
- **The unusual starting position — triage, not invention.**
  [`idempotency-capability.md`](../archive/retired-tracks.md) opened with "*What's
  already true: nothing*" — every question novel, no sibling to mirror. Events is
  the reverse: §7 is a large, opinionated, internally-consistent spec. The
  settling work here is therefore **sifting** the settled majority (§3.0) from a
  small set of genuinely-open corners (§3.1–§3.5), not designing the surface from
  scratch. The risk this doc must not fall into is relitigating settled §7 prose;
  the risk it must not hide is treating the two hard, substrate-level unknowns
  (the fan-out substrate, per-publisher ordering on Cloudflare) as settled merely
  because §7 asserts an outcome for them — the fan-out substrate (§3.1) is now
  settled by argument, and per-publisher ordering (§3.4) deliberately stays
  open until it can be settled by evidence instead.

## 1. The theme

A context declares a typed fact with the `event` keyword; a handler in that
context emits it with `given Events`; zero or more **subscriber services** in
other contexts, declared `from Events(SomeEvent)`, receive each emission as an
ordinary typed handler invocation. The event *type* is the topic — no string
topic names — and a subscriber may refine which emissions it receives with a
structural pattern on the payload, mirroring the auth pattern already on
services. Emission is fire-and-forget and releases at handler commit; an aborted
handler emits nothing. Delivery is at-least-once; effectful subscribers dedup
via the already-shipped `Idempotency` capability keyed on the runtime
envelope's `eventId` — though only to that capability's current limit, since
its **in-memory** provider does not survive a crash, so a duplicate delivered
across an isolate restart is not deduped until the durable provider (a future,
unfiled track, §3.6) exists (see the threat model, §6).

The end state when this track retires: `event` declarations, `given Events`
emission, and `from Events(...)` subscription (pattern-refined, with envelope
metadata and additive versioning) are real, checked surface with their one
first-party provider, and §7's `PaymentConfirmed` worked example (lines 166–241)
compiles and runs end to end — **minus** replay/backfill, which §3.6 moves to a
follow-on track.

## 2. Why a track (the ADR 0076 trigger)

- [x] **Multi-increment.** The core is naturally four-to-five separable slices
  (§4): the emit/subscribe loop, pattern refinement, the envelope + idempotency
  idiom, and versioning. Each lands as its own increment proposal.
- [x] **Surface not yet fully settled.** Despite §7's richness, the *substrate*
  it lowers onto is an explicit open fork ("Queues with topic-as-queue routing,
  **or** a custom event-fanout DO", line 1321), and the per-publisher FIFO
  guarantee (§7, line 231) is asserted against a platform whose queues do not
  provide it for free — both need settling, one empirically.
- [x] **Correctness/safety boundary.** Events cross context boundaries carrying
  domain payloads (§7 permits opaque fields, line 180) to an open set of
  subscribers. A pattern-filter bug over-delivers; an owner-emission-enforcement
  bug lets a context emit another's facts; a broken ordering or dedup story
  double-applies effects. See the threat model, §6.

## 3. Open design questions

### 3.0 — What §7 already settles (recorded so the settling pass does not reopen it)

Treated as **committed by the design notes**, not open questions for this track:

- **Type-as-topic routing** (§7, line 229) — every emission of `E` is offered to
  every subscriber of `E`; no explicit topic names, no namespace/typo risk.
- **Owner-only emission** (§7, line 180) — the compiler enforces that only the
  declaring context emits a given event type; `Events` is parameterised by the
  publishing context.
- **Release-at-commit** (§7, line 196) — emission releases at handler commit, an
  aborted handler emits nothing; identical semantics to outbound agent calls
  ([ADR 0106](../decisions/0106-async-message-send.md)).
- **Subscribers are services, not agents** (§7, line 280) — broadcast has no
  agent address; a subscriber service routes to a specific agent from the payload
  (`Orders(e.orderId).…`).
- **Subscriber failure isolation** (§7, line 282) — each subscription is an
  independent handler invocation with its own atomic transaction; one fault does
  not propagate to sibling subscribers.
- **Additive versioning by field defaults + breaking-by-convention** (§7, lines
  245–256, 278) — old wire events fill missing fields from pure default
  expressions; renames/narrowings introduce a new versioned event type, not a
  language feature.
- **The at-least-once safety floor** — subscribers taking effects on receipt
  dedup on `env.eventId` via the shipped `Idempotency` capability (§12, line 241;
  [`idempotency-capability.md`](../archive/retired-tracks.md)). This track *reuses*
  it; it does not rebuild it.

### 3.1 — The fan-out substrate on Cloudflare — SETTLED (the load-bearing fork)

**The question.** §7's lowering note offers a fork without choosing it: emit
"maps to Queues with topic-as-topic routing, **or** a custom event-fanout DO for
higher-fanout scenarios" (line 1321). This is the hard, expensive-to-reverse
decision of the whole track, because **three** of §7's asserted properties all
hang off it: per-publisher FIFO ordering (§3.4), subscriber failure isolation
with independent transactions, and (for the follow-on track) replay from a
durable log.

**The two candidates and their tension.**

- **Queue-per-topic (topic-as-queue).** One Cloudflare Queue per event type;
  each subscriber a consumer. Cheap, reuses the shipped `on queue` consumer path
  and `QueueResult` verdict ([ADR 0078](../decisions/0078-queueresult-typed-verdict.md)).
  But a single queue fanning out to *N* independent subscribers, each with its
  own at-least-once retry and its own dead-letter policy, is not what a plain
  consumer queue is — it needs a per-subscriber offset/ack, which a shared queue
  does not natively give. Failure isolation (§7 line 282) is the specific casualty:
  one subscriber's retry must not re-deliver to siblings that already acked.
- **Fanout DO.** A Durable Object owns the subscriber registry and the fan-out,
  giving per-subscriber delivery state and a natural home for per-publisher
  ordering (a DO is single-threaded) and, later, the replay log. But it is new
  runtime machinery, a scaling chokepoint at high fan-out, and it puts a DO on the
  emission hot path.

**Settled.** Slice 0 lowers emission onto the **fanout-DO** shape, because it is
the only one of the two that can *honestly* deliver §7's per-publisher ordering
and failure-isolation guarantees rather than approximate them — and the
follow-on replay track (§3.6) needs a durable log-owner anyway, so the DO is not
throwaway. The queue path stays a documented future optimisation for the
high-fanout, ordering-relaxed case, not slice 0's substrate. Recorded as
[ADR 0284 — events-fanout-substrate](../decisions/0284-events-fanout-substrate.md)
— the **foundational ADR landed before slice 0** (§5); everything else
composes above it. This settles the substrate choice itself; per-publisher
FIFO as a *verified* guarantee (§3.4) was a separate empirical decision, now
also settled — scoped narrower than this section's rationale implies (see
§3.4).

### 3.2 — Admitting Events to the closed protocol set — SETTLED, precedented

**The question.** Events is a sixth service protocol. The protocol set is
**closed** ([ADR 0079](../decisions/0079-protocols-closed-set.md)), so admitting
`from Events(...)` is a deliberate extension of that closed set, not a neutral
addition. The `on event` handler shape must join the per-protocol handler-shape
checking that already rejects, e.g., a queue handler returning a `Response`
(§7 line 160).

**Why this is mostly mechanical.** `from <protocol>` on the service header ships
([ADR 0077](../decisions/0077-service-protocol-on-header.md)), and `from
WebSocket` already proves a `from`-protocol carrying a bundle of associated
surface (the WebSocket track). Events differs in one real way: `from Events(E)`
**parameterises the protocol by an event type**, and (§3.3) by a pattern — no
shipped protocol takes a type argument on the header. That parameterisation is
the genuinely new grammar/checker slice-0 work; the closed-set membership itself
is a one-line extension. Recorded as
[ADR 0285 — events-protocol-set-extension](../decisions/0285-events-protocol-set-extension.md).

### 3.3 — Subscription pattern refinement — SHIPPED (slice 1), scoped narrower than asserted

**The question.** `from Events(E { region: Domestic, .. })` filters emissions by a
structural pattern (§7 lines 198–227). §7 asserted the pattern is type-checked
against the event shape at compile time, enforced by the runtime before the
handler runs, *and* that its statically-known fields are available in the body
(`e.region` is statically `Domestic`).

**What §7/this section originally claimed, verified false by direct
inspection of the code, before slice 1 was built.** "Mirrors the auth pattern
on services" and "reuses the shipped refined-pattern and nested-payload
machinery" (citing multi-actor sum dispatch, [ADR 0090](../decisions/0090-multi-actor-sum-dispatch.md);
authorisation invariants, [ADR 0091](../decisions/0091-authorisation-invariants-refinement-actors.md);
and the pattern machinery of ADRs [0169](../decisions/0169-nested-payload-patterns-and-match-arm-guards.md),
[0252](../decisions/0252-or-patterns.md), [0253](../decisions/0253-refined-patterns.md))
— none of that machinery fits. The shared `Pattern` AST has six variants,
none a record/field pattern; ADR 0169 Decision F explicitly deferred record
patterns as their own future slice; `Pattern::Variant` requires a sum-type
tag and an `event` is a plain record, not a sum. Separately, static
narrowing of `e.region` has no mechanism either — the closest shipped
analogue (ADR 0253's refined patterns) is a runtime guard only, and static
narrowing "waits on §2.5.4 (refinement propagation), which is still the
specification's largest open question."

**Shipped scope: filtering only, no static narrowing.** Slice 1 introduces a
small, **Events-local** pattern node (`EventPattern`), deliberately separate
from the shared `Pattern` enum — no exhaustiveness obligation, no shared-enum
blast radius, honest about being new surface. `e`'s type is **not** narrowed
in a matching handler body; only delivery is filtered. This amends
[ADR 0286](../decisions/0286-events-pattern-dispatch-deliver-and-filter.md)'s
"no bespoke matching engine is introduced for Events" claim — **its
deliver-and-filter decision remains correct and unchanged**: the fan-out
mechanism still delivers every emission of `E` to every subscriber
unconditionally; the guard lives entirely inside the subscriber's own
generated handler method (`emit_service`), covering all three delivery
paths (Cloudflare Workers, Bundle/node, Bundle/browser) in one edit.
Recorded as [ADR 0286 — events-pattern-dispatch-deliver-and-filter](../decisions/0286-events-pattern-dispatch-deliver-and-filter.md)
(the deliver-and-filter decision) and the `events-subscription-pattern-local-node`
ADR (the amendment; `design/pending/events-subscription-pattern-filtering.md`,
pre-stamp — the bespoke-node correction and the no-narrowing scope decision).

### 3.4 — Per-publisher FIFO: the contract vs. what Cloudflare delivers — SETTLED empirically (scoped narrower than asserted)

**The question.** §7 asserts a specific ordering *contract* (line 231): events
from the same publishing agent are delivered to each subscriber in emission
order; across publishers, no ordering. The publisher is the *agent*, not the
context. This composes with the atomic-handler invariant (events within a handler
release at commit in emission order; events from successive handlers of one agent
release in handler order).

**Settled, by measurement, and narrower than asserted.** The deploy track's
discipline applies directly here — it settled "Cloudflare resolves bindings
at upload — a hard barrier, not a soft nicety" **empirically**, not by
assertion ([ADR 0193](../decisions/0193-multi-context-deploy-ordering.md)),
and its headline finding was "the assumption was wrong." This guarantee got
the same treatment: `bynkc/tests/events_ordering_workerd.rs` runs a
two-context project on real `workerd` under two `wrangler dev` processes and
measures three cases. **Holds:** emissions within one handler body (one
batch, one sequential delivery loop); successive, sequentially-awaited
invocations of one agent (each flush completes before the next begins).
**Does not hold:** two *overlapping* invocations of the same agent —
interleaving was observed in every trial run. The mechanism: the fan-out DO
(§3.1) is a **stateless router** with no storage operation and no
`blockConcurrencyWhile`, so it yields at every delivery instead of gating on
one; the publisher's own flush happens *after* `commitState` reopens that
agent's storage gate, so two concurrent invocations can have both flushes in
flight at once with nothing to serialise them. Single-threaded DO execution
prevents parallelism, not interleaving across `await`-separated batches — the
§3.1 rationale ("a DO can sequence a publisher's emissions") holds for a
*single* batch, not across concurrent ones. Recorded as the
`events-per-publisher-fifo-scope` ADR,
`design/pending/events-per-publisher-fifo-scope.md` (pre-stamp), including
the decision **not** to change the emitter to close this gap (a substrate
redesign, not a documentation increment — see §6 for the consequence a
subscriber needs to know).

### 3.5 — The cross-build schema registry — CLOSED (shipped as slice 3c, #980)

**The question, as it stood.** §7 specifies "the compiler maintains a schema
registry across builds, computing the version from the type's structural
shape" (line 258), emitting a schema-evolution report when a version changes,
with an *automatic*, non-hardcoded `env.schemaVersion` — and, critically, that
"explicit `@schema(N)` annotations... are available for teams that want to pin
versions; the compiler verifies the declared version against what the schema
would otherwise warrant" (line 260). Slice 3b (#978) shipped only the
annotation half, with nothing to verify it against; this section tracked the
remaining verification half as open.

**What shipped.** A committed `bynk.schema.lock` at the project root (TOML,
atomic write — the same discipline as `bynk.deploy.lock`, a third,
un-extracted copy of that pattern; extraction would need a new crate below
`bynk-emit`, since both existing copies live in crates that depend on it —
named as follow-up debt, not fixed here), auto-written by `bynkc compile`'s
directory build and by `bynk dev`/`bynk deploy`'s build step — **not** by
any in-memory, fixture, or LSP compile (`CompileOptions::schema_registry`,
off by default: `bynkc/tests/e2e.rs` compiles hundreds of fixtures in place,
and an unconditional write would litter a `bynk.schema.lock` into every one
of them). Every build reconciles each event's current field shape against
its stored entry: unchanged keeps the version, a purely additive change (new
fields, all defaulted; nothing removed, retyped, or newly required) auto-
bumps it by one, and anything else — a field removed, retyped, added
without a default, or one that lost a default it had — fails the build
(`bynk.event.non_additive_schema_change`). A declared `@schema(N)` is now
genuinely **verified** against the computed value
(`bynk.event.schema_version_mismatch` on disagreement) rather than trusted
outright, closing the exact gap 3b left open. A brand-new event, or an
existing one's first compile after this slice, baselines silently at its
current `@schema(N)`-or-`1` — no migration hazard for what 3b already shipped.

*Where* it lives and *how* a version is computed were resolved, not by
reusing [ADR 0200](../decisions/0200-cross-context-contract-hash.md)'s
canonical form as first guessed possible, but with a purpose-built shallow
per-field snapshot: `canon_named_in`'s record rendering carries no signal
for default-presence, so it cannot tell an additive change (new field, has a
default) from a breaking one (new field, none) — they perturb an opaque
hash identically. The registry's own diff needs that distinction directly.
The reconcile step also has to run *before* emission (inside `run_checks`,
not after `compile_project` gets its result back) — `EmitProjectCtx.event_
schema_versions` is populated during the same per-unit pass that emits
TypeScript, before `compile_project` ever sees a finished build to gate a
write on. Only the write itself is deferred to a clean build. See this
slice's own ADR for the full account, including the rejected full-registry-
without-an-opt-in design and the deferred CI "fail instead of write" flag.
Slice 4 (`via schema(...)`) still does not depend on this — it already had
a real integer to match against from 3b.

### 3.6 — Replay / backfill — SPLIT OUT to a future track (the precedent move; recorded)

**Decision.** Event replay — a new subscriber "backfilling from log history",
the runtime upgrading old wire events to the current schema on read (§7 lines
243, 276) — is **not delivered by this track.** It moves, together with the
inherited actors **Q8 (replay/ordering)**
([#260](https://github.com/accuser/bynk/issues/260)), to a separate, future,
currently-unfiled track.

**Why split, not carry.** Three reasons, exactly paralleling
[`idempotency-capability.md`](../archive/retired-tracks.md) §3.2's durable-provider
split:

1. **It drags in an unbuilt dependency this track cannot satisfy.** Replay-safe
   subscribers dedup on `env.eventId` across a crash and a re-delivery window
   measured in the *log's* retention, not a request retry — which wants the
   **durable** `Idempotency` provider. Only the **in-memory** provider shipped;
   the durable one was itself deferred to a future unfiled track
   (`platform_capability_overrides.md`; `idempotency-capability.md` §3.2/§3.3).
   Events' replay story therefore depends on a provider that does not exist yet.
   Carrying replay here would make this track's completion hostage to a second
   unbuilt track.
2. **It needs a durable event log — its own substrate design.** §7 asserts
   backfill works but never says what stores the log, who owns retention, or the
   window. That is a substrate decision as heavy as §3.1 and genuinely unspecified
   — the definition of "not settled enough to carry."
3. **The live core is coherent and shippable without it.** Emit → subscribe →
   pattern-filter → envelope → additive-version is a complete, useful pub-sub
   system on its own. Replay is an additive capability on top, not a precondition
   for the core to be correct.

**What this track still owes replay (so the split is honest, not a dodge).** The
envelope (§4 slice 2) must carry `eventId` and `schemaVersion` from day one, and
the fan-out substrate (§3.1) must not foreclose a durable log — the §3.1 fanout
DO is chosen partly because it is the natural future log-owner. This track ships
the *seams* replay will need; it does not ship replay. Recorded as
[ADR 0287 — events-replay-out-of-scope](../decisions/0287-events-replay-out-of-scope.md)
— durable and citable rather than implicit in a diff.

## 4. Candidate slice decomposition

MVP-first. Each slice is an ordinary increment proposal, a sub-issue of the spine,
`accepted` before build.

- **Slice 0 — the emit/subscribe loop.** `event` declarations (owner-only, §3.0),
  the `Events` capability with `emit` (release-at-commit), and pattern-less
  `from Events(E)` subscriber services delivering via the §3.1 substrate,
  deliver-and-filter. Opens the closed protocol set to Events (§3.2) and adds the
  event-type-parameterised `from Events(E)` header grammar. The smallest thing
  that proves one publisher → one subscriber across contexts. Proven against a
  real `tsc --strict` project fixture (a two-context emit/receive), not
  golden-diffed alone.
- **Slice 1 — subscription pattern filtering — SHIPPED.** `from Events(E {
  field: value, .. })` (§3.3), via a small **Events-local** pattern node, not
  the shared `Pattern`/refined-pattern machinery (§3.3 records why). Delivery
  filtering only — the handler's parameter is **not** statically narrowed to
  the matched values (that half of §3.3's original claim is deferred, pending
  §2.5.4 refinement propagation). Multi-field patterns compose with AND; a
  pattern-less subscriber still matches all.
- **Slice 2 — the envelope + the idempotency idiom — SHIPPED.** `EventEnvelope`
  (`eventId`, `publisherId`, `emittedAt`, and `schemaVersion` reserved for slice
  3) passed as an `on event` handler's optional, user-declared second
  parameter; the documented `env.eventId` → `Idempotency.dedup`/`remember`
  convention for effectful subscribers (§12, corrected against the
  capability's real API — no new call-shape sugar was built). `publisherId`
  is the emitting context, not the emitting agent, amending §7 (a plain
  keyless service handler can emit with no agent identity to report).
  `eventId`/`emittedAt` are minted once per emission, before fan-out
  duplicates it to every subscriber. This is where the idempotency track's
  parked "event-subscriber sugar" (its own §7 possible slice 1) actually
  lands — settled as documented convention, not bespoke syntax.
- **Slice 3a — event field defaults — SHIPPED.** Default expressions
  (`field: T = expr`) on `event`-declared record fields (§3.0's
  `RecordField.init`, already parsed, previously unused outside agent `store`
  fields): static, pure, checked against the field's declared type; used only
  on deserialisation, so an old wire event missing a defaulted field's key
  falls back to the default instead of failing structural-mismatch. A default
  on a **non**-event record field is now a compile error (previously silently
  parsed and dropped). Lowers to the field's **wire** (JSON) form, not its
  in-memory form — the one correction this slice made to its own accepted
  proposal (#972): a qualified value-level reference (as agent-state defaults
  use, via `BodyMode::StaticInit`) would not resolve in a subscriber's own
  regenerated codec module, which only ever imports the publisher's *types*.
  No `env.schemaVersion`/registry dependency — see §3.5's narrowing.
- **Slice 3b — manual `@schema(N)` event versioning — SHIPPED.** An optional
  `@schema(N)` annotation on an `event` declaration (`N` a positive `Int`
  literal), embedded verbatim into `env.schemaVersion` at emission; absence
  still means version `1`, byte-identical to every event before this slice.
  Author-asserted, not derived — no persisted cross-build state, no drift
  detection, no evolution report (§3.5's narrowing explains why: that half
  would make `bynkc` the first-ever compiler command with committed state,
  out of proportion to this increment). A malformed `@schema` (non-positive,
  non-literal, wrong arity, labelled, or duplicated) is a compile error; any
  annotation name on an event other than `schema` is too.
- **Slice 3c — additive versioning's automatic-detection half — SHIPPED
  (#980).** The cross-build schema registry (§3.5): a committed
  `bynk.schema.lock`, auto-written on a clean `bynkc compile`/`bynk dev`/
  `bynk deploy` build; a version bump auto-detected from a purely additive
  structural shape change; a declared `@schema(N)` now verified against the
  computed value instead of trusted outright; a non-additive change (field
  removed, retyped, added without a default, or one that lost its default)
  fails the build rather than silently versioning. The registry's own git
  diff across a PR is the evolution report — no separate artefact.
- **Slice 4 — `via schema(N)` version-aware dispatch, literal only —
  SHIPPED (#985).** A `via schema(N)` clause after a `from Events(...)`
  header's closing `)`, matched against `env.schemaVersion` by exact
  equality; independent of the payload pattern (a service may carry
  either, both, or neither). Nested inside the `Events` protocol's own
  grammar arm, not a free-standing clause — `via` on `http`/`cron`/
  `queue`/`websocket` is a syntax error, not a checker diagnostic.
  **Narrowed at proposal time:** range patterns (`via schema(2..)`,
  `..v`, `v1..v2`) and the generalised `via <field>(pattern)` grammar are
  split to slice 4b ([#990](https://github.com/accuser/bynk/issues/990)) — no range-pattern or range-literal syntax
  exists anywhere in bynk (the `..` token exists only as a record-pattern
  rest marker), so ranges are a full new grammar/AST/parser/checker/
  emitter surface, disproportionate to bundle with the literal case. No
  cross-subscriber ambiguity check: two sibling subscribers with the same
  or overlapping `via schema(...)` coverage both independently fire,
  exactly like slice 1's payload pattern already does — the design notes'
  clean V1/V2-or-later worked example *reads* mutually exclusive but was
  never statically enforced as such, before or after this slice.

**Not slices of this track** (moved to the future replay track, §3.6): replay /
backfill-from-log, the durable event log substrate, and the inherited actors Q8
([#260](https://github.com/accuser/bynk/issues/260)). Recorded there, not here,
so this track's scope stays honest about what it delivers — the same discipline
`idempotency-capability.md` applied to its durable provider.

**Ordering is not a slice.** Per-publisher FIFO (§3.4) is a *property* every slice
must preserve; it is now established and verified empirically against the
chosen substrate, scoped to what the substrate actually delivers (§3.4).

## 5. Front-loaded ADR candidates

The load-bearing, hard-to-reverse decisions to land up front (ADR 0076/0167's
"foundational ADRs"). Four of the five landed as ADRs 0284–0287 ahead of any
slice-0 code; the fifth — deliberately deferred until there was something to
measure — has now landed too, recorded in
`design/pending/events-per-publisher-fifo-scope.md` (pre-stamp; its ADR
number is assigned at merge).

- **[Written] The Events fan-out substrate** (§3.1, `events-fanout-substrate`) —
  records the fanout-DO-vs-queue choice and *why*, because per-publisher
  ordering, subscriber failure isolation, and the future replay log all depend
  on it and it is the most expensive decision to reverse. **This is the one
  that had to land before slice 0 — it now has.**
- **[Written] Events joins the closed protocol set; the header takes a type
  argument** (§3.2, `events-protocol-set-extension`) — records extending
  [ADR 0079](../decisions/0079-protocols-closed-set.md) to a sixth protocol and
  the new event-type-parameterised `from Events(E)` header shape (no prior
  protocol parameterises on a type).
- **[Written] Per-publisher FIFO is a verified guarantee, scoped to what was
  measured** (§3.4, `events-per-publisher-fifo-scope`) — records the
  empirical fixture that backs the ordering claim on the chosen substrate, in
  the deploy track's evidence-not-assertion tradition
  ([ADR 0193](../decisions/0193-multi-context-deploy-ordering.md)), and the
  finding that the guarantee holds only within a batch and across
  non-overlapping calls — not across concurrent invocations of one agent.
- **[Written] Subscription pattern dispatch is deliver-and-filter, not
  server-side pre-filtering** (§3.3, `events-pattern-dispatch-deliver-and-filter`)
  — deliver-and-filter is the committed semantics, server-side pre-filter a
  later semantics-preserving optimisation. **This ADR's other claim — "no
  bespoke matching engine is introduced for Events," asserting the pattern
  would reuse auth/refined-pattern machinery — was found false once slice 1
  was actually built** (the shared `Pattern` enum cannot represent a record
  field filter; see §3.3).
- **[Written] The subscription pattern is an Events-local node, amending the
  above** (§3.3, `events-subscription-pattern-local-node`,
  `design/pending/events-subscription-pattern-filtering.md`, pre-stamp) —
  records the correction: a small, purpose-built `EventPattern` node, not a
  reuse, and the accompanying scope decision that slice 1 ships filtering
  only, no static narrowing of the handler's parameter.
- **[Written] The envelope's `publisherId` is the emitting context, amending
  §7** (slice 2, `events-envelope-publisher-id-amendment`,
  `design/pending/events-envelope-and-idempotency-idiom.md`, pre-stamp) —
  `Events.emit` is legal from a plain, keyless service handler with no agent
  identity to report, so a context-scoped identifier is what's actually
  available uniformly at every legal emission site.
- **[Written] The envelope is minted once, at emission, via bare
  `crypto.randomUUID()`/`Date.now()`** (slice 2,
  `events-envelope-mint-once-and-minting-mechanism`, same pending file) —
  one emission must produce one `eventId` shared by every subscriber it
  fans out to; not routed through `given Clock` (would break every
  existing fixture that emits with `given Events` alone).
- **[Written] The idempotency idiom is documented convention, correcting
  §12's API mismatch** (slice 2, `events-idempotency-idiom-is-documented-
  convention`, same pending file) — §12's `dedup(on:, expiresAfter:)?`
  never shipped; the real two-call `dedup`/`remember` API, keyed on
  `env.eventId`, is the idiom. Also names a pre-existing, general
  field-access checker limitation found while proving the
  no-capability-envelope case (reproduces on an ordinary cross-context
  record field too — out of scope for this track).
- **[Deferred to slice 3] Event schema versioning + the cross-build registry**
  (§3.5) — slice-3 ADR; records where the registry lives and whether it reuses
  the ADR 0200 normal form. Not a slice-0 blocker, so not written now.
- **[Written] Replay is out of scope** (§3.6, `events-replay-out-of-scope`) —
  records the split decision itself, so it is durable and citable rather than
  implicit in a diff, and names the future track and its dependency on the
  durable `Idempotency` provider.

## 6. Threat model

**Asset.** The event payload in transit from a publishing context to an open set
of subscribers — potentially carrying domain data (§7's `PaymentConfirmed`
amount/customer/region), including opaque fields a foreign subscriber may hold but
not introspect (§7 line 180) — and the two correctness guarantees the mechanism
makes: that *only* the owning context emits a given fact, and that a subscriber
receives *exactly* the emissions its pattern admits.

**Adversary / failure modes.**

- **Over-delivery via a pattern-filter bug.** A subscriber whose generated guard
  admits emissions its declared pattern should exclude sees data outside its
  intended slice — a cross-context data exposure if the excluded emissions carry
  data that subscriber should not react to. *Mitigation, shipped and verified
  (slice 1, #966):* the filter is generated from the type-checked pattern and
  enforced as the first line of the subscriber's own handler body, before any
  user code runs; deliver-and-filter keeps the guard in one generated place.
  `bynkc/tests/events_pattern_behaviour.rs` proves both directions in the same
  run — a matching emission is delivered, a non-matching sibling subscriber's
  emission is not — so the negative isn't asserted from reading the code alone.
- **Forged/cross-context emission.** A context emitting another context's event
  type would let it fabricate facts attributed to a peer. *Mitigation:* owner-only
  emission is statically enforced (§3.0, §7 line 180) — this is a compile error,
  not a runtime check, and is the primary boundary guarantee.
- **Duplicate delivery double-applying effects.** At-least-once means a subscriber
  can receive the same event twice. *Mitigation, shipped and demonstrated
  end to end (slice 2, #968):* the `Idempotency` capability keyed on
  `env.eventId`, proven combined with Events for the first time by
  `bynkc/tests/fixtures/positive/961_events_envelope_idempotency_dedup` —
  but note the **in-memory** provider does not survive a crash, so a
  duplicate across an isolate restart is *not* deduped until the durable
  provider (the future track, §3.6) exists. This is the same accepted-gap
  posture idempotency itself shipped with, named here not hidden.
- **Mis-ordered delivery corrupting state.** A subscriber that assumes emission
  order (e.g. a `Created` before an `Updated`) breaks if the substrate reorders.
  *Mitigation, and its real scope (§3.4, empirically measured):* order is
  preserved within one emission batch and across successive,
  non-overlapping calls to one agent — **not** across *concurrent*
  invocations of the same agent, which the fan-out DO's stateless,
  yield-at-every-`await` delivery loop can interleave. Cross-publisher
  ordering is explicitly **not** guaranteed either. A subscriber that must
  not observe either of these reorderings needs its own sequence number in
  the payload; this is a design constraint the docs must call out, not
  something the runtime absorbs on the subscriber's behalf.
- **Payload validation at the boundary.** An event is a value crossing a trust
  boundary. **Found false and fixed post-slice-2, before slice 3 (#973):** on
  Cloudflare Workers, the payload and envelope crossed the entire delivery
  chain (mint → fan-out DO → `deliverEvent` → the receiving route) typed
  `unknown`, terminating in a bare `payload as any` cast — no
  `deserialise_*` was ever called, so nothing was actually validated on
  receipt. Fixed: both are now validated at the receiving `/_bynk/event/`
  route, the same validate-then-`400` shape `/_bynk/call/` has always used.
  **There is no dead-letter policy for Events, and none is introduced by this
  fix** — a malformed payload's `400` makes `deliverEvent` throw, caught by
  the fan-out DO's existing per-subscriber `try`/`catch`, which logs the
  failure and moves on to the next subscriber; that one subscriber's delivery
  is lost, siblings unaffected. Subscribers never defensively re-check a
  refinement the type already established. Recorded as the
  `events-boundary-validation` ADR, `design/pending/events-boundary-
  validation.md` (pre-stamp).
- **A missing key is not automatically "malformed."** **Amended by slice 3a
  (#972):** the bullet above still holds for a wrong-*shaped* or wrong-*typed*
  field — that is still a `400`. But a wire event missing the key for a field
  that declares a default is no longer treated as malformed at all: the
  subscriber's own codec substitutes the default before validation runs, and
  the request succeeds. Only a field with **no** declared default still fails
  structural-mismatch on a missing key, exactly as before this slice.
- **`schemaVersion` is now a real, verified value, not a hardcoded
  constant.** **Amended by slice 3b (#978), then closed by slice 3c
  (#980):** an event's `@schema(N)` annotation, if present, is verified
  against the version the cross-build schema registry computes from the
  event's field-shape history rather than embedded verbatim and trusted; a
  disagreement is now itself a build failure (`bynk.event.schema_version_
  mismatch`), and an unsafe shape change (a field removed, retyped, added
  without a default, or one that lost its default) fails the build outright
  (`bynk.event.non_additive_schema_change`) rather than silently versioning
  past it.

## 7. Slice status

- [x] Slice 0 — emit/subscribe loop + closed-protocol-set extension (#939)
- [x] Slice 1 — subscription pattern filtering, deliver-and-filter (#966).
  No static narrowing (scoped narrower than §3.3 originally claimed).
- [x] Slice 2 — `EventEnvelope` + the `env.eventId` idempotency idiom (#968).
  `publisherId` is the emitting context (amends §7's per-agent framing);
  the idiom is documented convention, not new syntax (corrects §12).
- [x] **Defect, found and fixed post-slice-2, pre-slice-3 (#973):** on
  Workers, an event's payload/envelope had no runtime validation at all —
  fixed by generating a subscriber-side codec for a consumed-but-never-called
  publisher's event type, a hand-written envelope validator, and
  validate-then-`400` at the receiving route. See §6.
- [x] Slice 3a — event field defaults (#972). Wire-form, type-directed
  lowering — not `BodyMode::StaticInit` reuse (amends #972's own proposal;
  see §3.5, §4).
- [x] Slice 3b — manual `@schema(N)` event versioning (#978). Author-
  asserted, embedded verbatim into `env.schemaVersion`; no persisted state,
  no drift detection (see §3.5, §4).
- [x] Slice 3c — the cross-build schema registry (#980). `bynk.schema.lock`,
  auto-written on a clean build; auto-bump on a purely additive shape
  change; a declared `@schema(N)` now verified, not trusted (see §3.5, §4).
- [x] **Defect, found during slice 3a, fixed post-slice-3c (#977):** a
  consumed boundary type whose field is a built-in generic wrapping *another*
  consumed named type (`region: Option[Region]`) had that inner type emitted
  **unqualified** in a subscriber's own regenerated codec, so `tsc --strict`
  failed with `TS2304: Cannot find name 'Region'`. Not Events-specific — the
  same break reproduced on a plain `consumes` + cross-context **call** — and
  fixed generically: the context's own generic-instantiation pass walks the
  boundary types it *declares*, leaving a foreign type's instantiations to the
  qualified consumed pass that owns the `import type * as <ns>` alias. Slice
  3a's `Option[Int]` fixture sidestepped it; `Option[Region]` now pins it.
- [x] Slice 4 — `via schema(N)` version-aware dispatch, literal only
  (#985). Nested inside the `Events` protocol's grammar arm; no cross-
  subscriber ambiguity check (same policy as slice 1's payload pattern);
  range patterns split to slice 4b ([#990](https://github.com/accuser/bynk/issues/990), see §4).
- [ ] (Not a slice of this track) Replay / backfill + actors Q8 — future track (§3.6)

## 8. Done when

- [x] `event` declarations, `given Events` emission, and pattern-filtered
  `from Events(...)` subscription are checked surface with their one
  first-party provider; §7's `PaymentConfirmed` example compiles and runs
  end to end (a real `tsc --strict` project fixture), **minus** replay.
  **Slice 0 done** (#939): `event`/`given Events`/unpatterned
  `from Events(E)` ship, verified by a real `tsc --strict` two-context
  project fixture on both Cloudflare and Bundle targets. **Slice 1 done**
  (#966): the subscription pattern filters delivery — **not** narrowed
  static typing of the matched fields, which §7's own worked example implies
  but which slice 1 deliberately does not build (§3.3).
- [x] Events is a member of the closed protocol set
  ([ADR 0079](../decisions/0079-protocols-closed-set.md)) with the
  event-type-parameterised header; the per-protocol handler-shape check rejects a
  malformed `on event` handler. Shipped in slice 0 (#939).
- [x] Per-publisher FIFO (§3.4) is backed by an **empirical** integration fixture
  on the chosen substrate, not asserted (`bynkc/tests/events_ordering_workerd.rs`).
  The guarantee entering the spec is **narrower** than §7 originally asserted:
  ordered within a batch and across non-overlapping calls to one agent, not
  across concurrent invocations of one agent.
- [x] The envelope carries `eventId`, `publisherId`, `emittedAt`, `schemaVersion`;
  effectful subscribers dedup on `env.eventId` via the shipped `Idempotency`
  capability (no new dedup mechanism built here). **Slice 2 done** (#968):
  `on event(e: E, env: EventEnvelope)`'s optional second parameter, minted
  once per emission (`bynkc/tests/events_envelope_behaviour.rs` proves
  identical ids within one emission, distinct ids across two);
  `schemaVersion` is structurally present but hardcoded (real, author-
  controlled values are slice 3b). `publisherId` amends §7 to the emitting
  context, not the emitting agent (a plain service handler can emit with no agent
  identity); §12's idempotency example is corrected against the
  capability's real API.
- [x] Event field defaults — **slice 3a done** (#972): `field: T = expr` on an
  event's own fields, checked static/pure against the declared type; a
  default on a non-event record field is a new compile error
  (`bynk.event.default_outside_event`); a wire event missing a defaulted
  field's key deserialises with the default instead of failing
  structural-mismatch (`bynk.event.bad_field_default` covers an invalid/
  unconstructible default). Proven cross-context on a real `workerd`
  (`bynkc/tests/events_boundary_workerd.rs`), not just golden-diffed.
- [x] An event may assert its own wire schema version. **Slice 3b done**
  (#978): an optional `@schema(N)` annotation (`N` a positive `Int`
  literal) embeds verbatim into `env.schemaVersion` at emission; absence
  still means version `1`, byte-identical to every event before this slice.
  Author-asserted at the time — no cross-build state, no drift detection, no
  evolution report yet (slice 3c closed that gap). A malformed
  `@schema`, or any other annotation name on an event, is a new compile
  error (`bynk.event.bad_schema_version` / `bynk.event.unknown_annotation`).
  Proven behaviourally in-process, across both a service-handler and an
  agent-handler emission (`bynkc/tests/events_schema_version_behaviour.rs`)
  — a live-`workerd` boundary test cannot observe the mint site this slice
  changes, since it posts a hand-authored envelope directly to the
  receiving route, bypassing emission.
- [x] `env.schemaVersion` computed for real (structural-shape drift
  detection, not just author assertion) + the cross-build registry ship.
  **Slice 3c done** (#980): `bynk.schema.lock`, auto-written on a clean
  `bynkc compile`/`bynk dev`/`bynk deploy` build (never by an in-memory,
  fixture, or LSP compile); unchanged shape keeps the version, a purely
  additive change auto-bumps it, anything else fails the build
  (`bynk.event.non_additive_schema_change`); a declared `@schema(N)` is
  verified against the computed value (`bynk.event.schema_version_mismatch`
  on disagreement) rather than trusted. The registry file's own diff across
  a PR is the evolution report — no separate artefact was built. Proven at
  two levels: `bynk-emit::project::schema_registry`'s own unit tests cover
  every reconciliation-table row; `bynkc/tests/events_schema_registry_
  behaviour.rs` proves the registry actually reaches the `Events.emit` mint
  site across three real compiles of one on-disk project (baseline, additive
  bump, blocked non-additive change).
- [x] `via schema(N)` dispatch ships. **Slice 4 done** (#985): a positive-
  `Int`-literal `via schema(N)` clause after `from Events(...)`'s closing
  `)`, matched against `env.schemaVersion` by exact equality; independent
  of the payload pattern. A bare `on event(e: E)` handler needs no
  declared `env` to use it — `emit_service` threads a synthetic envelope
  parameter into the generated method whenever the protocol carries a
  `via schema(...)` clause and the handler didn't declare its own,
  positionally matched by widening the same envelope-forwarding condition
  at both delivery paths (`workers.rs`'s compose wrapper, `project.rs`'s
  Bundle dispatch closure). No cross-subscriber ambiguity check — the same
  deliver-and-filter policy slice 1's payload pattern already established.
  Proven behaviourally (`bynkc/tests/events_schema_dispatch_behaviour.rs`):
  the same two subscribers, compiled once at each of two schema versions,
  each version's single emission reaching only its matching `via`
  clause — with the matching subscriber for version 1 declaring no `env`
  at all, the only way to prove the synthetic-parameter plumbing actually
  threads the value through. Range patterns (`via schema(2..)`) are
  slice 4b ([#990](https://github.com/accuser/bynk/issues/990)).
- [ ] The doc is explicit that **replay/backfill and the actors Q8**
  ([#260](https://github.com/accuser/bynk/issues/260)) are **not** delivered by
  this track — named as a future track with its durable-`Idempotency` dependency,
  not silently dropped.
- [x] Foundational ADRs (§5) written that do not depend on slice-0 code —
  the substrate ADR (§3.1, ADR 0284), the closed-protocol-set extension
  (§3.2, ADR 0285), the deliver-and-filter dispatch commitment (§3.3, ADR
  0286), and the replay split (§3.6, ADR 0287) — landed **before** slice 0.
- [x] The ordering-evidence ADR (§3.4, `events-per-publisher-fifo-scope`)
  landed, backed by the empirical fixture, before per-publisher FIFO is
  documented as shipped — in its measured, scoped form.
  Spec-in-place updates for the Events protocol are a per-slice follow-on.
  **On retire:** remove this doc; append its closing summary to
  [`../archive/retired-tracks.md`](../archive/retired-tracks.md); close the spine
  (`Closes #<n>`).
