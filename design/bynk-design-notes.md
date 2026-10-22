# Bynk — Design Notes

*Working draft — 9 May 2026*

**Bynk** is a service-tier application language: functional, message-driven, immutable, asynchronous with FIFO message ordering. The first compilation target — and the one whose primitives shape the architecture — is Cloudflare Workers and Durable Objects. The platform abstraction is designed so other targets with similar primitives could in principle be supported; the architecture itself is informed by Cloudflare's specific affordances rather than platform-agnostic.

The name is the Cornish word for a cairn — a built-up rocky landmark on the moors. Applications built in this language are bynks: small individually-meaningful primitives stacked into coherent architectures that endure and that orient the traveller across difficult terrain.

This document captures the design decisions made so far and the questions still open. It is intended as a living artefact, edited as the design develops.

A note on syntactic forms: the specific notation shown throughout (glyphs like `<-`, `:=`, `given`, `by`, the choice of keywords, the shape of actor declarations) is illustrative and provisional. Worked examples should be read for the architectural shapes they express, not for the particular syntax. What is committed at this stage is the conceptual structure; the surface that realises it will be revisited.

---

## 1. Purpose and Scope

Bynk is for the part of a backend system that holds the domain, owns state, and orchestrates responses to requests. It is data-oriented and organisational in spirit: it expresses how a system is *structured*, not how individual computations are squeezed.

**In scope.** Stateful application logic, data orchestration, request/response and message-driven workflows, multi-tenant patterns, real-time interaction (e.g. WebSockets), background jobs.

**Out of scope.** UI, systems-level work, scripting, heavy compute, embedded code, browser-side code, CLI utilities.

The genre is **service-tier application language**: neither domain-specific (too narrow) nor general-purpose (too broad) nor platform-bound (too vendor-specific). Source files use the `.bynk` extension.

## 2. Principles

The language is an expression of organisation: separation of concerns made syntactic. Where conventional languages permit good and bad architecture and rely on developer discipline, this language makes the bad versions inexpressible. Architectural commitments — what holds state, what is pure, what crosses a boundary — are visible at the level of glyphs, not buried in types or conventions. The pedagogical implication is direct, and it is for the working developer rather than the classroom: a practitioner writing this language cannot avoid producing well-organised systems, and comes to understand the architecture *as they use it* — the expressible program tends to be the correct one, and each refusal names the invariant it guards. Bynk is a production service-tier language that teaches at the point of use, not a teaching language (see [`bynk-positioning.md`](bynk-positioning.md)).

What follows is the broader set of principles that have emerged through the design and that govern future decisions. Each is a load-bearing commitment with a stated benefit; corollaries and consequences are referenced in the body of the document.

### Foundation

**Good architecture should be inexpressible to violate.** Correctness becomes structural rather than disciplinary.

**The language pays off where developers expect friction.** Indexing, idempotency, capability injection, deployment configuration — first-class affordances rather than vigilance or boilerplate.

### Architectural commitments

**State has a single named owner.** Every piece of state lives in one agent, subject to known invariants and atomic-commit discipline; no orphan state, no shared mutability.

**Failure has two kinds, not one spectrum.** Outcomes are typed parts of contracts; faults are abnormal events that abort. Conflating them produces fragile error handling.

**Effects are tracked through types.** Pure code is genuinely pure; side effects are visible at the call site; reasoning about behaviour is local.

**Atomicity is local; coordination is explicit.** Single-agent commits are atomic; cross-agent flow uses messages, events, and compensation. The architecture refuses guarantees the platform cannot deliver.

**Idempotency is the system-wide default.** At-least-once delivery is safe by construction; non-idempotency is signposted at the call sites where it occurs.

### Surface conventions

**Architectural facts live in declarations.** Filters, patterns, authentication, invariants, capabilities — all visible at the contract boundary. Bodies operate inside their assumptions rather than re-checking them.

**Architectural cost is visible at the call site.** Awaits, fault-handling, cleanup registration, idempotency, read-modify-write, indexing — each signposted at the point where it matters.

**One canonical way to do each thing.** Smaller surface area to learn; idioms become identifiable patterns rather than dialectal variants.

### Design discipline

**Conservative start; extend when pressure emerges.** Add only what the architecture demonstrably needs. Convention preferred over feature where the convention covers the case.

**Source describes the architecture; build describes the mapping.** Architectural concepts live in source; deployment mapping is build configuration generated from source. The source is the deployment specification.

### Layered surface

Bynk is a coherent system but it is a large surface to meet all at once: bounded contexts, the actor/service/agent split, opaque vs transparent exports, outcomes vs faults, atomic handlers, idempotency annotations, on-abort stacks, the storage type taxonomy, query algebra, event subscription patterns, schema versioning. A developer needs only a fraction of this to write a useful program. The architecture is layered so that the harder concepts compose on top of the simpler ones rather than being entangled with them, and the documentation is organised to reflect this. This is ordinary good ergonomics for any developer meeting a large language — not a concession to a classroom (see [`bynk-positioning.md`](bynk-positioning.md)): Bynk teaches as it is used, and the layering is what keeps "as it is used" from meaning "all of it, immediately".

*Foundations — "core Bynk".* With these you can write a complete, useful service: bounded contexts as namespaces; the service/agent split; actor declarations for authentication; value types with opaque-versus-transparent visibility; basic handlers returning `Result[T, E]`; atomic-handler semantics; `Cell` and `Map` storage with the `:=` and `.update(fn)` write forms; cross-agent calls, made by constructing an agent with its key and calling a handler (`Counter(id).bump()`), from a service or from another agent. (Earlier drafts, and the examples later in this document, spell an agent reference `Ref[A]`; that type was never built — the keyed-construction form is the shipped one.) This is the smallest set that reaches a working HTTP-fronted service with a persistent agent backing it. Most day-to-day work belongs at this layer.

*Coordination — when state crosses contexts.* Capabilities and `given` clauses; the `Idempotency` capability for at-least-once safety; the `Sagas` capability for multi-step compensation; event emission and pattern-based subscription; the `traverse`/`parTraverse` vocabulary for cross-agent fan-out. This is where most production code lives, and where the architecture begins to pay off most visibly relative to conventional approaches.

*Advanced — answers to specific problems.* Schema versioning with `via schema(...)` and field defaults; the full query algebra and indexing semantics; `Log`-shaped time-window patterns; compilation strategy and deployment tuning; first-party capabilities for cross-cutting concerns (the `Sagas` capability for durable workflow compensation is the canonical example, with others emerging as the language matures). These are answers to specific advanced concerns; most code doesn't reach them, and most developers don't need them for a long while.

This layering is intentional. The architecture supports progressive disclosure structurally, not as instructional retrofit: the simpler layers are complete in themselves, and the harder ones compose on top without changing how the simpler layers work. Documentation organisation and tooling presentation should reflect the same gradient — you see the foundations first and meet the rest only when you need it.

## 3. Vocabulary

The language's vocabulary aligns with established architectural traditions. The alignment is deliberate, but it is also emergent — the same distinctions, expressed in the same words. A DDD practitioner, a UML modeller, and a hexagonal-architecture reader all find their concepts already in place.

| Term            | Role                                                            | Aligns with                                          |
|-----------------|-----------------------------------------------------------------|------------------------------------------------------|
| Bounded context | Coherent piece of the domain wrapping actors, services, agents, types, and consumed capabilities | Bounded context (DDD); ports-and-adapters context (Hexagonal); rough analogue of a module/package in conventional terms |
| Actor           | External participant *outside* the system boundary              | UML actor; "frameworks and drivers" (Clean Arch)     |
| Service         | Boundary handler; translates external input to internal message | Application service (DDD); port/adapter (Hexagonal)  |
| Agent           | Internal participant with state and identity                    | Aggregate (DDD); entity / use case (Clean Arch)      |
| Value type      | Immutable data shape, no identity                               | Value object (DDD)                                   |
| Storage type    | Access-pattern slot owned by an agent                           | (novel; no direct architectural analogue)            |
| Capability      | Typed handle for outside-world access                           | Effect; injected dependency                          |
| Message         | Typed communication between services and agents, or among agents| Command / query (DDD)                                |
| Event           | Typed fact published by one context, subscribed to by zero or more others; in-system pub-sub vocabulary | Domain event (DDD)                                   |

## 4. Architectural Layering

Three concentric regions, mirroring hexagonal and clean-architecture diagrams.

- **Outside the system: actors.** The external world. Browsers, queue producers, alarms, webhooks, schedulers, time itself. Actors are not declared as runnable entities; they are the named other parties the system contracts with.
- **On the boundary: services.** Translate untyped, untrusted external stimulus into typed internal messages. Own everything to do with the wire — protocol semantics, parsing, authentication, shape validation, idempotency keys at the protocol level, response shaping, status codes, headers — and nothing more.
- **Inside the system: agents.** Hold state, respond to typed messages, communicate through well-defined channels. Own everything to do with the domain — invariants, business rules, workflows, compensations, state transitions — and nothing more.

The two layers stand in deliberate symmetry. Services own everything wire-shaped; agents own everything domain-shaped. The boundary between them is the typed message (passing inward, having shed its wire-specific dressing at the service) and the typed outcome (passing back, ready to be shaped into a wire-appropriate response). Each layer has all and only the information it needs to do its job, and neither leaks into the other. A service that contained business rules would be reaching across that boundary; an agent that knew about HTTP status codes would be doing the same. The rule is enforced by what each layer is *given*: services receive parsed messages and actor identities; agents receive typed inputs and capabilities. Neither receives the other's vocabulary.

This layering is *per-context*. Each bounded context (Section 8) has its own outside, its own boundary, and its own inside; what plays the "inside" role within one context can play the "outside" role relative to another. The architectural layering recurses without breaking, and the principles that govern it apply uniformly at each level.

### Idempotency as a system convention

At-least-once delivery is the floor for cross-handler messaging in this architecture (Section 12). Commands retried by the runtime, events replayed to recovering subscribers, compensations dispatched during unwinding — each can arrive more than once at its receiver. The only structurally honest response is that receivers be **idempotent**: applying an operation twice produces the same observable state as applying it once.

This is a top-level architectural commitment, not a per-feature footnote. The language and the platform both lean on it. The `Idempotency` capability (Section 12) makes at-least-once safe by construction. Saga compensations via the `Sagas` capability (Section 13) require idempotent targets. Storage operations prefer idempotent shapes (Section 10) and signpost non-idempotent ones at the call site. Where a domain operation cannot be expressed idempotently — issuing a charge, sending a notification, appending an audit entry — the convention is that the caller supplies a deterministic identifier (an idempotency key) and the receiver dedupes against it; the `Idempotency` capability gives every handler the same first-class mechanism for this rather than a programmer-written boilerplate.

The convention shapes architectural decisions throughout a codebase. Handlers are designed so that their effects depend only on the operation's identity, not on how many times it has been delivered. Cross-agent calls accept idempotency keys; receivers honour them. Compensations target operations that survive being retried. Event subscribers either work on naturally idempotent state or dedupe against the event envelope's identifier. A reader of a well-structured application in this architecture should expect idempotency as the default, with non-idempotent operations explicit where they exist.

## 5. Kinds of Thing (Declaration Keywords)

A small fixed set of declaration forms, each naming exactly one kind of entity. The keyword is the first signal a reader receives about what they are looking at.

- `type` — value shape (record, ADT)
- `actor` — external participant declaration (a contract; never instantiated by the system itself)
- `service` — boundary handler
- `agent` — internal participant with state
- `fn` — function. Pure at the module level. When declared inside an agent, may declare capabilities via `given` and access the agent's storage; in that case it shares the calling handler's atomic transaction and serves as a private effectful helper for decomposing handler logic without crossing transaction boundaries.
- `on` — handler clause within a service or agent
- `store` — storage field within an agent

## 6. Actor Declarations as Contracts

An `actor` declaration is a *contract type*, not a runnable entity. It tells the language what the system expects of an external party: how that party proves it is who it claims to be, what identity to attach to its messages once verified, and what assumptions the boundary should make about the messages it sends. The declaration is partly a description and partly a recipe — the compiler uses it to generate the verification logic that services would otherwise have to write by hand.

The information carried by an actor declaration falls into a few natural categories:

- **Identity.** What this party is *in the system's terms* once verified — a `UserId`, a `TenantId`, an opaque session, or `Unit` for genuinely anonymous parties. This is the value that the service hands to the agent layer when verification succeeds.
- **Authentication scheme.** How a request claiming to come from this party proves itself: bearer token verified by some verifier, HMAC signature with a shared secret, mTLS, signed webhook, or `None` for anonymous. The scheme is what the compiler turns into actual verification code.
- **Authorisation invariants.** Properties that must hold for the party to be considered this actor — for example, an `Admin` is an authenticated user who additionally carries an `admin` claim. Invariants belong in the type because they are load-bearing for the boundary contract.
- **Idempotency expectations.** Whether messages from this party carry idempotency keys, whether duplicates should be deduplicated, whether retries are expected.
- **Replay and ordering.** Webhook senders famously retry; alarm firings are at-least-once; queue producers may deliver out of order. The actor type can declare what the runtime should expect, which the service then handles.

A service consumes an actor type by naming it on a handler clause; the handler is invoked only if the actor's contract is satisfied. By the time the handler body runs, the boundary's verification work has already happened, the payload has been parsed into a typed message, and the actor's identity is available as a typed value to pass downstream. Anonymous public endpoints are not exempt from this discipline — they declare a `Visitor` actor with an authentication scheme of `None`. Multi-actor routes take a sum type, dispatching structurally on which actor verified rather than via runtime branching on the presence of credentials.

The principle this realises is that *trust* — historically the most discipline-dependent and most error-prone concern in service-tier code — becomes visible domain modelling. A reviewer can tell at a glance which routes admit which parties, what the verification scheme is, what identity propagates downstream, and whether anything anonymous is allowed in. The boundary becomes a thing you read rather than a thing you have to remember to check.

The language ships an initial set of authentication schemes (bearer token, HMAC signature, mTLS, none, internal) that cover common cases. Whether and how this set is extensible — for example, by allowing user-defined verification capabilities — is recorded as an open decision; the conservative starting position is a closed set that can be opened later.

## 7. Services and Protocol Composition

A service is the bounded context's interface to the platform — the only thing in the context the platform talks to. Its job is to take whatever the platform delivers, verify the actor's contract, parse the payload into a typed message, and either produce a typed response or hand the work off to an agent. Different protocols deliver stimuli of different shapes, but the structure of the service-and-actor composition is uniform across them, because the architectural role is the same regardless of what flavour of stimulus the platform is offering.

The word *Protocol* here is used in the Objective-C / Swift / Clojure sense — a named set of handler shapes that conforming services must satisfy — not in the network-engineering sense. `from HTTP` is a conformance claim: a service's handlers must match the shape the protocol requires, and the compiler verifies it the way Swift verifies that a class conforms to a declared protocol. The lineage runs through Smalltalk's informal protocols (sets of related messages a class was expected to respond to), which Objective-C formalised, Clojure adopted explicitly, and Python extended structurally in PEP 544.

A useful distinction this framing makes available is between the **protocol** — what the language sees, the contract a service conforms to (handler shapes, mandatory and optional handlers, response requirements) — and the **transport** — what the platform provides (Cloudflare Queues, AWS SQS, an HTTP server, raw TCP, an MQTT broker). A `service Jobs from Queue` is portable across queue transports the way a service from HTTP is portable across HTTP runtimes. The protocol layer is a language concern; the transport layer is a platform concern. This is the Roc-style application/platform separation applied one level deeper into the boundary.

Five protocols cover the bulk of service-tier work:

- **HTTP.** Request/response, per-call authentication, rich actor types (User, Admin, Visitor, signed Webhook), typed responses.
- **Queue.** One-way fire-and-forget from the producer's perspective; no response, only an ack or retry. Producers carry `auth = Internal` if internal; signed-webhook or similar if external. Cloudflare Queues deliver in batches, which the language exposes both per-message and per-batch rather than hiding.
- **Cron.** Scheduled trigger with no payload. The actor is the platform `Scheduler` with `auth = Internal`. The compiler turns the cron expression into a runtime trigger.
- **Alarm.** Not strictly a service. Alarms are runtime-generated typed messages addressed directly to a specific agent; the agent uses an `Alarms` capability to schedule a future wake-up and exposes a handler the runtime invokes when the time arrives. This preserves handler uniformity: an alarm handler has the same shape as any other handler, and a unit test can fire it directly.
- **WebSocket.** The case where the boundary hands a long-lived resource to an agent. The service performs the HTTP upgrade and authentication; the connection is then transferred to an agent that owns the persistent state. From that point on, incoming WebSocket frames arrive at the agent as ordinary typed messages, indistinguishable in shape from messages invoked by other agents or by tests.
- **Events.** In-system pub-sub for cross-context decoupling. A typed event declaration describes the shape of a published fact; an `Events` capability lets agents emit; services declared `from Events(SomeEvent)` subscribe and receive each emission as a typed handler invocation. The actor for event-mediated calls is the platform's event bus, with `auth = Internal`. Multiple subscribers receive the same event independently; zero subscribers is valid (the emit becomes a no-op). Delivery is at-least-once via the runtime. This is how contexts decouple from one another: a publisher announces facts, subscribers react, neither knows the other.

The pattern that emerges is sharp: **services are stateless protocol adapters; agents are stateful participants; long-lived runtime resources flow from services to agents at the moment of acceptance**. The boundary does the validation work that requires knowing about the protocol; the agent does the domain work that requires holding state. WebSocket connections, alarm schedules, queue subscriptions, and cron triggers all sit on the runtime side of the architectural line, but they appear in the language as typed messages (for things the runtime delivers to agents) or capabilities (for things agents ask the runtime to do). The actor-as-contract idea applies uniformly — Schedulers, JobProducers, Users, and signed Webhooks are all parties whose trust scheme and identity are declared at the boundary.

A useful asymmetry to keep in mind: response shape. HTTP services synthesise typed responses; queue services produce acks (and side effects in agents); cron services produce nothing visible; alarms and WebSocket events are pure side-effect-shaped. That asymmetry is real and deserves to be visible in handler signatures — a handler that returns a value implies a protocol that carries one back. The compiler can reject a queue handler that returns a `Response` and reject an HTTP handler that omits one.

### Events in depth

Of the protocols, Events is worth treating more fully because it is the first with two-sided participation (publishers and subscribers) and the only one whose filtering mechanism is itself a language feature rather than a property of the transport.

**Declaration.** Events are typed value-types declared in their owning context using the `event` keyword.

```
context commerce.order {
  event PaymentConfirmed = { 
    orderId: OrderId, 
    userId:  UserId, 
    amount:  Money,
    region:  Region,
    at:      Instant
  }
}
```

The `event` keyword distinguishes events from plain `type` declarations: only events can be emitted and subscribed to, and the compiler enforces that only the owning context emits a given event type. Visibility is always transparent (subscribers need to read the payload), with opaque fields permitted — a subscriber from another context can hold an `OrderId` field but not introspect it, which is the existing visibility model applied unchanged.

**Emission.** Events emit through a platform capability supplied to the owning context.

```
on markPaid(authId: AuthId) by InternalCaller given Events, Clock {
  paymentRef := Some(authId)
  status     := Paid
  Events.emit(PaymentConfirmed { 
    orderId: id, userId: customer, 
    amount:  total, region: regionOf(customer),
    at:      Clock.now 
  })
}
```

The `Events` capability is parameterised by the publishing context — you can only emit events declared locally, statically enforced. Emission is fire-and-forget; the actual release happens at handler commit. An aborted handler emits nothing. This is the same release semantics as outbound agent calls and composes with the atomic-handler invariant.

**Subscription with pattern-based refinement.** A subscriber is a service `from Events(SomeEvent)` declared in whichever context wants to react. The subscription may specify a structural pattern that the event must match — refinement at the subscription level, mirroring the auth pattern on services.

```
context commerce.notifications {
  consumes commerce.order
  consumes chat.rooms

  service OnDomesticPayment 
        from Events(commerce.order.PaymentConfirmed { region: Region.Domestic, .. })
        given Rooms: Ref[chat.rooms.Room] {
    on event(e: commerce.order.PaymentConfirmed) {
      -- e.region is statically Region.Domestic here
      Rooms(RoomId.forUser(e.userId)).systemMessage("Payment confirmed")
    }
  }

  service OnInternationalPayment 
        from Events(commerce.order.PaymentConfirmed { region: Region.International, .. })
        given Rooms: Ref[chat.rooms.Room], Fx: Fx.Converter {
    on event(e: commerce.order.PaymentConfirmed) {
      let local = Fx.convert(e.amount, to: User.localeOf(e.userId))
      Rooms(RoomId.forUser(e.userId)).systemMessage("Payment confirmed: \(local)")
    }
  }
}
```

A subscription without a pattern matches all events of the type; a partial structural match filters to events satisfying it. The `..` syntax indicates "rest of the fields, unconstrained." The pattern is part of the subscription declaration, so it's visible in the service signature. The compiler type-checks the pattern against the event type's shape; the runtime enforces the match before the handler runs; the handler body assumes the filter has passed.

This mechanism mirrors auth dispatch on services and reuses the same architectural moves: structural, declarative, visible, and enforced before the handler runs. The handler body doesn't defensively re-check what the architecture has already established. The benefits are parallel too: type explosion is avoided (one event per fact, refined at subscription), multi-dimensional refinement composes (subscribers refine on any combination of discriminator fields), statically-known fields are available in the handler body (the compiler knows what's true), and server-side filtering becomes possible where the platform supports it (with deliver-and-filter as a transparent fallback).

**Routing: type-as-topic.** The event type *is* the topic. Every emission of `PaymentConfirmed` is offered to every subscriber of `PaymentConfirmed`; each subscriber's pattern determines whether the offer becomes a delivery. There are no explicit topic names, no namespace questions, no typo-routing risk. If a distinction is structurally important (international versus domestic), it is expressed as a discriminator field on the event and refined at subscription; if it is important enough to be a different fact, it is a different event type.

**Ordering: per-publisher.** Events emitted by the same agent are delivered to each subscriber in emission order; events across publishers have no ordering guarantee. The publisher is the emitting agent, not the bounded context — agents are the unit of state and effect, so they are the unit of ordering. This is the same guarantee as cross-agent commands (per-sender FIFO) and composes with the atomic-handler invariant: events within a handler release at commit in emission order; events from successive handlers of the same agent release in handler order.

**Envelope metadata.** Every event implicitly carries runtime-managed metadata — event ID, publisher ID, emission timestamp — accessible via an envelope passed alongside the event payload.

```
on event(e: commerce.order.PaymentConfirmed, env: EventEnvelope) {
  -- env.eventId, env.publisherId, env.emittedAt available if needed
}
```

For at-least-once delivery with idempotency, subscribers use the `Idempotency` capability (Section 12) with `env.eventId` as the dedup key. The runtime dedupes against the envelope identifier transparently; no manual cache-based tracking is required.

**Event versioning and replay.** Event types evolve. New fields get added; subscriber populations change at different rates; new subscribers backfill from log history and need to see old events. The language commits a small surface for this that extends the existing event mechanism rather than introducing a separate versioning system.

*Additive evolution through field defaults.* The common case is adding a field. Record fields on event types may carry default expressions evaluated at deserialisation when the field is absent from the wire format:

```
event PaymentConfirmed {
  amount:        Money,
  customer:      CustomerId,
  paymentMethod: Option[PaymentMethod] = None,    -- added later
  region:        Region = Region.Domestic,        -- added later
}
```

Old serialised events deserialise with missing fields filled from their declared defaults. Subscribers see a uniformly-shaped event; most don't need to know which version they're handling. Default expressions are pure (no capabilities); the compiler verifies that. Defaults apply specifically to event-type fields here, but the mechanism naturally extends to any record type that benefits from forward-compatible deserialisation.

*Schema version in the envelope.* The runtime tracks a `schemaVersion: Int` per event type, exposed on `env.schemaVersion`. The compiler maintains a schema registry across builds, computing the version from the type's structural shape: adding a field with a default bumps the version; type changes that aren't forward-compatible bump the version and require explicit handling. The build emits a schema-evolution report each time a version changes, making the lineage visible during review. Explicit `@schema(N)` annotations on the event type are available for teams that want to pin versions; the compiler verifies the declared version against what the schema would otherwise warrant.

*Version-aware dispatch via envelope patterns.* Subscribers that need to handle versions differently extend the existing pattern-refinement mechanism from payload to envelope using a new `via` clause:

```
service OnPaymentV1
      from Events(commerce.order.PaymentConfirmed { region: Domestic, .. })
      via schema(1)
      given ... { ... }

service OnPaymentV2OrLater
      from Events(commerce.order.PaymentConfirmed { region: Domestic, .. })
      via schema(2..)
      given ... { ... }
```

The `via` clause introduces a pattern on the envelope, parallel to the payload pattern inside `from Events(...)`. Schema patterns may be literal versions, ranges (`v..`, `..v`, `v1..v2`), or `_` for any. A subscriber without a `via` clause receives any version, equivalent to `via schema(_)`. The grammar generalises — `via <field>(pattern)` is the shape — so future envelope-pattern extensions (matching on publisher, on emission time, etc.) compose without grammar changes; for v1 only `via schema(...)` is committed.

*Replay.* A new subscriber backfilling from log history reads events in their original wire format; the runtime upgrades them to the current schema on read using the declared defaults; subscribers see them as if just emitted. The envelope's `schemaVersion` reflects the original emission schema, available to subscribers that care. No migration scripts required for additive changes.

*Breaking changes by convention.* Renames, type-narrowing changes, and semantic redefinitions don't fit additive evolution. The architectural answer is to introduce a new event type with a versioned name (`PaymentConfirmedV2`), emit both during a transition window, migrate subscribers, and retire the old type once the log's retention window has cleared it. This is a procedural pattern rather than a language feature; the existing event-type and subscription-pattern machinery already supports two independent topics with overlapping content during a transition.

**Subscribers are services, not agents.** Events are broadcast and don't carry an addressing key for a specific agent; agents are addressable individually. Mixing the two creates routing questions ("which Order receives this PaymentConfirmed?"). The clean rule: subscribers are services that may then route to specific agents using addressing derivable from the event payload (`Orders(e.orderId).markFulfilmentReady()` and so on). This keeps the broadcast-versus-address-routed distinction structural.

**Subscriber failure isolation.** Subscribers are independent. If one subscriber faults processing an event, others still receive it. Each subscription is a separate handler invocation with its own atomic transaction; faults in one do not propagate to another.

### Boundary validation via refined types

Refined types (Section 15) carry their constraints into runtime validation automatically. Wherever a value crosses a trust boundary — an HTTP request body, a URL parameter, a header, a queue message, an event payload, or rehydration from durable storage — the framework attempts to construct the refined type from the deserialised input and validates against the type's refinement. A `POST /redeem` whose body is declared with `code: VoucherCode` rejects requests where `code` doesn't match the `VoucherCode` regex; the failure is a structured response (HTTP 400 with field-level error detail) and the handler body never runs. Event subscribers whose payload types include refined fields reject malformed events to the platform's dead-letter policy; consumers see only valid data. Storage rehydration validates refined fields on agent start, catching schema corruption before it reaches application code.

The handler body, having received a validated payload, never defensively re-checks what the type system has established. The same refinement that gives the type its compile-time identity gives the runtime its validator. No separate validation library, no schema-to-type drift, no possibility of a handler running on data that violates the type's contract.

## 8. Bounded Contexts

The unit of architectural organisation in this language is the **bounded context**, in the DDD sense — a coherent piece of the domain with its own vocabulary, its own state-bearing entities, its own boundary. The name is more weight-bearing than "module" or "package": this is not just code grouping or namespacing, it is the architectural primitive that wraps actors, services, and agents into a coherent whole. A team writing in this language thinks in bounded contexts the way they think in agents and services.

A bounded context contains:

- **Value types** — the vocabulary of this context's domain
- **Actor declarations** — the external parties this context contracts with
- **Services** — the context's boundary interface, translating external stimulus to typed messages
- **Agents** — the context's state-bearing participants, holding the domain's invariants
- **Pure helpers** — functions that don't fit inside an agent
- **Consumed capabilities** — what the context needs from the platform or other contexts
- **Provided capabilities** — what the context offers to others (rare in application contexts; the main job of platform contexts)

Each context exports a *public interface*: the value types it shares as common vocabulary, the agent contracts (handler signatures and the message types they refer to) other contexts may invoke, the services it exposes to actors, and any capabilities it provides. Storage layouts, private helpers, and internal types stay inside. Cross-context references go through these declared exports — an agent in one context invoking an agent in another does so through `Ref[OtherContext.SomeAgent]`, where `OtherContext.SomeAgent` is the exported contract of the agent. Internals remain private.

Exports themselves come with a *visibility* that controls how other contexts may use them. The visibility governs the *read side* — what consumers can see of the type's structure — but never grants construction authority. Construction of a context-owned type is always a prerogative of the owning context. Two visibility levels:

- **Opaque export.** The type's name is published; its structure is hidden. Other contexts can hold values of the type, store them, copy them, pass them as arguments, compare them by `==`. They cannot inspect (no field access, no pattern matching) and they cannot construct. An opaquely-exported type is a *token* outside its context — a value with identity and equality but no readable structure.
- **Transparent export.** The type's structure is published for *inspection*. Other contexts can read fields, destructure records, pattern-match on sums. They still cannot construct. A transparently-exported type is *readable data* outside its context — consumers can examine it, react to its shape, translate it, but not mint new values of it.
- **Private** (not exported). The type stays inside the context. Storage layouts, helper types, anything not part of the public contract.

**Exports are a context mechanism. Commons don't export.** This distinction is architecturally important. The `exports` clause and its visibility levels (opaque, transparent) apply specifically to contexts. Commons, by contrast, do not have visibility levels — they *mix in*. Whatever a commons declares becomes part of every using context's vocabulary, with no boundary to cross. The two mechanisms answer different questions:

- *"What vocabulary do I want as part of my context's language?"* → `uses` a commons. Mixin brings declarations in.
- *"What types should callers of my services be able to interact with?"* → `exports` from a context. Visibility determines the contract — opaque for tokens, transparent for readable data, private for what stays inside.

A context typically uses both. It `uses` one or more commons to draw in shared vocabulary (Money, identifiers, units), then `exports` selectively to govern what its callers see. Mixin governs vocabulary in; exports governs contract out.

This is a stronger encapsulation than typical opaque-types-with-public-constructors. The principle is that **a value of a context-owned type is only minted by handlers in that context** — never by external callers via factory functions, never by user code constructing literals. Cross-context interaction is through service operations (the context's handlers, invoked by peers) and events (published from inside, received outside as facts). The boundary is sealed against external minting.

For domain identifier types (`Sku`, `UserId`, `OrderId`, `ReservationId`), opaque is the natural choice — the identifier means something inside the context that owns it, and outside is a token to be held and handed back. For sum types representing outcomes or errors (`ReserveOutcome`, `OrderError`), transparent is the natural choice — consumers need to match on variants to react appropriately, but they don't need to construct new ones. For value-record shapes used at the boundary (`Cart`, `Receipt`), transparent is also typically the right answer (the framework's deserialiser constructs them on receipt; consumers read them on return). For internal types, private.

The construction rule has a clarifying consequence: when one context needs to react to another's domain error, the idiomatic pattern emerges naturally:

```
<- Rooms(room).reserve(rsvId, dates).mapErr(e =>
  match e {                                  -- read foreign type (commerce.rooms)
    DatesUnavailable(conflicts) => 
      RoomUnavailable(conflicts)             -- construct local type (hotel.bookings)
    InvalidDateRange => 
      InvalidDates                           -- construct local type
  }
)?
```

Pattern-matching reads the foreign error (admitted by its transparent export); the right-hand sides construct the local error (admitted because the local error type lives in the current context). This is the Anti-Corruption Layer expressed in the type system — and it is the *only* shape cross-context error translation can take, since constructing the foreign error from the local context would be a construction-rule violation. The right pattern is enforced by the language.

For types that genuinely belong to no single bounded context — `Money`, `CurrencyCode`, `Date`, calendrical types, broadly-shared value types — the language provides a separate construct: the **commons**. Commons are a peer of bounded contexts, not contexts themselves, with strictly different rules. They bundle types and pure functions, and they're **mixed into** any context that `uses` them — the commons's declarations are brought into the using context's scope as if locally declared. Each using context becomes a defining context for its mixed-in commons types, so construction of those types is admitted everywhere they're used. Where contexts are behavioural (agents, services, capabilities, state) and sealed (per the encapsulation principle), commons are pure (only types and pure functions; the compiler enforces) and unbounded (no behavioural surface; any context can `uses` them; values flow freely because structural shapes are identical across mixin sites). The distinction is architectural: a context owns a piece of the domain's behaviour and seals its types behind service operations; a commons captures a deliberately-shared kernel of vocabulary that becomes part of every using context's local language. Naming the two constructs differently (`context` vs `commons`) and the two import relationships differently (`consumes` for behavioural dependency on a context, `uses` for vocabulary mixed in from a commons) keeps the architectural reality visible at every declaration site.

Bounded contexts remain **flat** under this model. The dotted qualified names that group contexts (`commerce.inventory`, `commerce.payment`) reflect subdomain organisation — team ownership, file structure, conceptual clustering — but impose no architectural relationships. A commons named `commerce.money` is not "for" `commerce.*` contexts any more than for any other context; any context anywhere in the project can `uses commerce.money` and have its declarations mixed in. The naming hierarchy is purely organisational, in keeping with DDD's strict position that bounded contexts have no containment relationship. The subdomain is a conceptual envelope (a problem-space partition); the bounded context is a solution-space unit; neither contains the other.

Tests are not a third top-level kind. A **suite** — declared with the `suite` keyword and naming the unit it targets — is an ordinary top-level declaration legal in any file alongside `context` and `commons` (Section 14). Test-ness is a property of the declaration, not of a distinct context kind: the build strips every `suite`, the test runner compiles and runs them, a suite reaches its target through the same surface any caller uses, substituting capability seams with `provides` and fabricating values with `Val[T]`. Suites do not deploy with production; they exist to verify the systems they target.

A principle follows from this: **actors are relative to a context, not absolute**. The system-as-a-whole has one boundary; each bounded context has its own. An entity is an actor with respect to some boundary — a browser is an actor relative to the system; an agent in one context is an actor relative to another. That generalisation gives a uniform mechanism for cross-context interaction. The called context treats the calling context as an actor and applies the same actor-as-contract machinery used for external parties: declared actor types, declared trust schemes (typically `auth = Internal` for in-system cross-context callers), and statically-checked contracts.

Two flavours of public interface emerge, sharing the same mechanism:

- **Service-mediated** for external actors — browsers, webhooks, queue producers. The full protocol applies (HTTP, queue, cron) with parsing, authentication, and idempotency at runtime; actor declarations use external schemes (`BearerToken`, `Signature`, etc.).
- **Contract-mediated** for cross-context callers — agents in sibling contexts in the same system. Typed RPC without wire-format overhead; actor declarations use `Internal`; the call is statically verified at compile time rather than re-validated at runtime.

The boundary is the same boundary in both cases. Only the auth schemes and the timing of verification differ. The DDD pattern of *context maps* — how a bounded context relates to its neighbours — becomes a piece of the program rather than a piece of documentation, and anti-corruption is *structural*: a context cannot be reached except through its declared exports by its declared actors.

Cross-context interaction has two semantic shapes, regardless of which mediation mechanism is used. **Commands** are targeted, imperative, and expect a specific receiver — `Orders(id).markPaid(authId)` is a command from one context to a specific agent in another. **Events** are broadcast past-tense facts — `PaymentConfirmed { ... }` is an event published by one context that any number of others may subscribe to. Commands are contract-mediated (typed RPC into a specific agent); events are service-mediated (typed publication into the Events protocol). The two compose: a context can publish events that others react to, and it can also issue commands when it needs a specific agent to do specific work. Together they cover both *coordinated* cross-context flow (commands) and *decoupled* announcement (events). The choice between them is a domain-modelling decision: use a command when the publisher needs a specific outcome from a specific receiver; use an event when the publisher is announcing that something has happened and is indifferent to who, if anyone, reacts.

This composes cleanly with the architectural layering of Section 4. The outside / boundary / inside layering describes what happens *within* a single bounded context: that context's actors, that context's services, that context's agents. The bounded context is the wider unit. A system is one or more bounded contexts; each has its own layered architecture inside; cross-context interaction is mediated by the contract surface, not by reaching across boundaries. This matches DDD's bounded-context-with-anti-corruption-layer pattern naturally — each context has its own ubiquitous language, and translation between contexts happens at the seams when their vocabularies overlap.

The platform fits the same pattern. Cloudflare's runtime supplies capabilities like `Clock`, `Random`, `Http`, `Storage`, `Alarms`, `Queue`, `WebSocket`, organised into one or more *platform contexts*. Application contexts consume them by declaring the capabilities they need. This is what keeps the language runtime-portable: an application's bounded contexts depend on capability *contracts*, not on implementations, and a different platform (BEAM, Akka, Convex, custom) can supply the same capability surface. Platform contexts are conceptually no different from application ones; they happen to be supplied by the runtime rather than written by the developer.

Several things this opens that are deliberately deferred:

- Concrete syntax for declaring a bounded context, its imports, and its exports.
- The typical *granularity* of a context — whether one application has a few large contexts or many small ones is a question of practice rather than language.
- Whether contexts can be parametrised (ML-style functors). Probably not at this stage.
- Whether a context corresponds to a deployment unit. Likely not — multiple contexts can deploy together as one application — but this interacts with the platform's deployment model and is worth deciding deliberately.

What is committed at this stage is the *shape*: the bounded context is the architectural primitive at the organisational layer; it wraps actors, services, agents, types, and capabilities; it exports contracts and consumes contracts; the language enforces its boundaries the way it enforces the actor / service / agent split.

## 9. Separations Enforced

The language refuses to express the following, structurally rather than stylistically:

- **Identity vs value.** Values have no identity; only agents do. There is no construct that fuses the two.
- **Compute vs data.** Pure functions transform values; agents own data. There is no class that fuses both.
- **Schema vs access pattern.** Value types describe shape; storage types describe access pattern. They compose orthogonally.
- **Effect vs computation.** Pure functions cannot reach the outside world. Effects are typed capabilities, named in signatures, never ambient.
- **Shared mutable state.** Storage types may only appear as fields of an agent. No module-level mutables, no globals; everything addressable is an agent.
- **Implicit distribution.** Local work is sequential and atomic; cross-agent work is asynchronous and visibly so.
- **Application vs platform.** The program describes *what*; the platform decides *where*.
- **Invocation source vs handler logic.** Handlers are uniform under invocation source. An agent's handler is invoked identically whether the caller is another agent, a service that has just finished validating an external request, the runtime delivering a platform event (alarm, WebSocket frame, queue message), or a unit test harness. The agent never branches on origin. Source-aware concerns — authentication, schema validation, trust, idempotency — live at the boundary, expressed through actor types and service handlers; the agent owns only the typed response to typed input. This is what makes domain logic testable by direct handler invocation, refactorable across protocols, and reviewable as pure responsibility rather than as wire-bound code.
- **Service work vs domain work.** A service handler's responsibilities are boundary-shaped: shape validation (parsing the body into a typed message, which the type system handles automatically from the message type), authentication (via the actor type), dispatch to an agent, and response shaping. Business validation, multi-step orchestration, and compensation belong in agents, where per-handler atomicity makes them tractable. A service handler that orchestrates multiple agent calls in sequence is doing domain work in the wrong layer; the workflow should move into an agent handler, decomposed via private effectful helpers (`fn` with declared capabilities) that share the handler's transaction. The architectural rule that follows: *if the orchestration would benefit from atomicity, put it where atomicity is available* — inside an agent, not at the service.
- **State ownership.** Services are stateless. They have no `store` fields, no Cells, no Maps, nothing that survives across handler invocations. Where a service needs to consult or update state — idempotency tables, request audit, per-route counters, anything — the service delegates to an agent that owns the state. The agent may be created specifically for that purpose (e.g. an `IdempotencyTable` agent referenced via a `Ref` capability), or it may already exist for domain reasons. Either way, the service holds no state; the agent does. This keeps the per-context architecture diagram readable as "services in, agents below," and ensures every piece of state in the system has a single named owner subject to that owner's invariants and atomic-commit discipline.
- **Platform interface vs domain effect.** Services are the bounded context's interface to the platform — the only thing in the context the platform talks to. Every typed message arriving at the context (HTTP request, queue message, cron trigger, alarm, WebSocket frame, published event) enters through a service. Agents are the domain primitive — they hold state and produce domain effects (state changes, event emissions, cross-agent calls, capability use). The platform sees none of this directly; it only sees what services hand back as protocol responses, and what the runtime intercepts via capabilities and event emission. The diagonal where the language puts pressure: services do not do domain work, and agents do not do boundary-protocol work.

## 10. Storage Types (Working Set)

Storage types are the language's data abstractions. They are the only locus of mutation, the only things owned by agent identity, and the only constructs that talk to durable storage. Services are stateless (Section 9); when a service needs to consult or update state, it does so through an agent it references. Every storage field lives in some agent's body, subject to that agent's invariants and the handler-level atomic commit.

**All storage operations are Effect-typed**, returning `Effect[T]` and requiring `<-` to await. This makes storage cost visible at the call site (the same discipline applied to cross-context calls) and remains honest across compilation targets where storage may be genuinely async. The single ergonomic exception is `Cell`, where implicit dereference and `:=` assignment are syntactic sugar over the Effect-typed read and write operations — the compiler inserts the await automatically for the most common single-value idioms. All other storage types (`Map`, `Set`, `Log`, `Queue`, `Cache`) have no sugar; every operation site is an explicit `<-`. In-memory storage types (sync access, non-durable) are deliberately deferred from v1; agent state is durable, and local `let` bindings are the only sync state inside a handler.

The committed shapes:

- `Cell[T]` — single value, implicit dereference in expression position (`status` reads), with two write forms (see below)
- `Map[K, V]` — keyed collection, queryable, with idempotent and non-idempotent operations
- `Set[T]` — unordered collection of distinct elements, idempotent add and remove
- `Log[T]` — append-only, ordered, time-indexed
- `Queue[T]` — durable async stream
- `Cache[K, V]` — TTL-bounded
- `Ref[A]` — capability handle to an agent
- `Connection[F]` — typed handle to a runtime-managed resource; the first concrete instance is a WebSocket connection, parameterised by the server-frame type `F`. Has identity, has lifecycle managed by the platform (including transparent survival of agent hibernation), and can be passed as a typed message argument or stored in agent state. The general pattern — a typed handle to a runtime-managed resource — is referred to as `Held[T]`; further instances may appear as the platform's capability surface grows.

Refinement annotations (`@indexed(...)`, `@ttl(...)`, `@retain(...)`, `@bounded(...)`) add information about access pattern and constraints without dictating implementation.

### Write forms: `:=` vs `.update(fn)`

The architecture's idempotency convention (Section 4) shapes how writes are expressed. Two forms exist; the compiler enforces which is appropriate at each call site:

- **`cell := value`** — unconditional write. The new value is independent of the prior value. Idempotent on final state: applied twice produces the same observable state as applied once.
- **`cell.update(fn: T -> T)`** — read-modify-write. The new value is computed from the prior value. *Not* idempotent on final state: applied twice compounds the function. Used where the dependency on prior value is genuine (decrementing a counter, mutating a record field, advancing a state machine).

The compiler enforces the split: if the right-hand side of `:=` references the left-hand side (`available := available - qty`), it errors with a suggested rewrite to `.update`. This makes read-modify-write visible at every site where it occurs, which is exactly where retry-compounding risk lives. The `.update` form maps directly to the underlying platform's atomic-update primitives where the platform exposes them.

Within a single handler, handler atomicity means there is no race — both forms are equally safe locally. The discipline pays off across handler retries, sharded variants, and any future loosening of the per-agent serial execution model. It also pays off for pedagogy: a reader sees at a glance whether a write's result depends on what was there before.

### Map operations

Map operations sort by idempotency the same way:

- `map.put(k, v)` — unconditional put. Idempotent on final state (last-write-wins).
- `map.update(k, fn: V -> V)` — read-modify-write on existing entry. Fault if `k` is absent. Not idempotent.
- `map.upsert(k, default: V, fn: V -> V)` — read-modify-write with default-if-absent. Not idempotent.
- `map.remove(k)` — idempotent remove (no-op if `k` is absent).
- `map.get(k) -> Option[V]` — read, no effect.

For storage at the entry level, `map.put(k, v)` is the idiomatic unconditional write; the older pattern `map := map.insert(k, v)` is removed in favour of direct methods.

### Set operations

Set is a first-class primitive even though its implementation is a `Map[T, Unit]` underneath. The interface is what matters:

- `set.add(t)` — idempotent insert (no-op if `t` is already present).
- `set.remove(t)` — idempotent remove (no-op if `t` is absent).
- `set.contains(t) -> Bool` — read.
- `set.size: Int`, `set.isEmpty: Bool` — read.
- `set.union(other)`, `set.intersection(other)`, `set.difference(other)` — combinators returning a new `Set`.
- `set.asList -> List[T]` — ordered list for iteration; ordering is implementation-defined but stable within a transaction.

Equality on values of opaque and transparent types alike is **structural equality of the underlying representation**. Two `OrderId` values constructed from the same input compare equal, the same way two `Money` values with the same `amount` and `currency` compare equal. Opacity is a visibility constraint, not an equality semantics — external code cannot construct an opaque value from raw fields or pattern-match its representation, but it can compare two values via `==` and get the answer the type's definition would compute. Set membership and `==` apply this rule uniformly.

### Log operations

`Log[T]` is the one storage primitive whose write is *not* idempotent: `log.append(e)` produces a new entry on each invocation, so an at-least-once retry appends twice. Where this matters, the entry type should include a deduplication key (an event id, a request id, an operation id) and consumers should dedupe against it; alternatively, the appending handler should use the `Idempotency` capability (Section 12) keyed on the same identifier so the second invocation is suppressed before the append occurs. The non-idempotency is called out as the deliberate exception to the general convention because append-only sequence semantics require it.

### Query vocabulary

Storage types and in-memory collections share a combinator vocabulary documented in Section 11 (Query Algebra). Building a query against a storage type is pure (returns `Query[T]`); terminal operations execute the query and have storage-read effects. In-memory collection methods are eager and produce results immediately. The receiver's type tells the reader which kind of operation is at hand. Effectful iteration combinators (`traverse`, `parTraverse`, `traverseAll`, `parTraverseAll`) for cross-agent fan-out apply to in-memory `List[A]` and are also covered in Section 11.

### Refined types in storage

Storage types parameterised over refined value types (Section 15) carry their refinements through to durable storage. A `Cell[Money]` where `Money` contains a refined `CurrencyCode` field validates the constraint on every write; a `Map[VoucherCode, Voucher]` validates the key on every insert. Validation also applies on rehydration: when an agent's state is loaded from durable storage at startup or recovery, refined fields are validated against the current type definition, catching schema corruption or migration mismatches before application code runs. The same refinement that gives the type its compile-time identity gives storage its write-time and rehydration-time validation; there is no separate storage schema definition to maintain.

## 11. Query Algebra

Storage types and in-memory collections share a combinator vocabulary for reading and transforming data. The vocabulary is small and uniform; the receiver type determines evaluation timing.

### Lazy storage queries, eager in-memory operations

Operations on `Map`, `Log`, and `Set` storage primitives produce lazy `Query[T]` values; nothing executes until a terminal operation. Operations on `List`, in-memory `Set`, and in-memory `Map` are eager: each method returns its result immediately. The same combinator names (`filter`, `map`, `sortBy`, ...) appear on both, with semantics adjusted by receiver type. A reader knows from the type of the value being chained against whether they are building a query or transforming a collection: a chain against `Reservations` (an agent's `Map` field) builds a `Query`; a chain against a `List` already in scope runs eagerly.

A function that builds a query is pure (returns `Query[T]`); a function that executes one needs the storage-read effect, which agent storage fields carry automatically. Pure-construction-and-effectful-execution lines up with the lazy/eager split.

### `Query[T]` as a first-class type

The lazy value is nameable. A pure helper can return one for composition:

```
fn pendingReservationsExpiringBefore(t: Time) 
    -> Query[Reservation] given Reservations: Map[ReservationId, Reservation] {
  Reservations
    .filter(r => r.status == Pending)
    .filter(r => r.expiresAt < t)
}
```

Query construction is separable from query execution; the boundary is visible at the type. Domain-specific helpers can assemble query fragments and the handler that needs the result ultimately collects them. `Query[T]` is by reference identity — comparing two queries for value-equality is not meaningful, since they are computational descriptions rather than values.

### Agent-locality

Queries are scoped to a single agent's storage. Cross-agent data flow goes through message passing: a call to another agent's handler that returns data, not a query that reaches across the boundary. This preserves the architectural property that agent state is private (only the owning agent's handlers can read it), and structurally prevents the distributed-query failure modes that come with cross-machine joins.

If a handler needs data from multiple agents, it makes the cross-agent calls (each typed, each going through the per-sender FIFO machinery, each potentially asynchronous) and combines the results locally after they return. The query algebra applies to that local combination when the results are storage-shaped; for the more common case where results are `List`-shaped, the in-memory combinator vocabulary applies.

### Builder vocabulary

Builders return `Query[T]` (on storage) or the same collection type (on in-memory). The committed builders:

- `filter(p: T -> Bool)` — predicate selection
- `map(f: T -> U)` — transform each element
- `flatMap(f: T -> Query[U])` — bind, for one-to-many transformations
- `sortBy(f: T -> K)` — order by a key, where `K` has an `Ordering` instance
- `take(n: Int)` — limit
- `skip(n: Int)` — offset
- `distinct` — deduplicate (for `T` with structural equality)
- `distinctBy(f: T -> K)` — deduplicate by key

Joining and grouping take a final `into` combiner that names the result: bynk has no pair or tuple type, so the joined row exists only inside `into` ([ADR 0120](decisions/0120-join-group-combiner-form.md), v0.94). The arguments are positional; the names below are for reading.

- `join(other: Query[U], on: (T, U) -> Bool, into: (T, U) -> V) -> Query[V]` — general predicate join
- `joinOn(other: Query[U], left: T -> K, right: U -> K, into: (T, U) -> V) -> Query[V]` — equi-join on a value-keyable key, eligible for index acceleration
- `leftJoin(other: Query[U], left: T -> K, right: U -> K, into: (T, Option[U]) -> V) -> Query[V]` — preserve left, optional right
- `groupBy(key: T -> K, into: (K, List[T]) -> V) -> Query[V]` — partition in first-seen key order; terminal `.collect` materialises to `List[V]`

On an in-memory `List[T]` the same builders return `List[V]`.

### Map key accessors (v0.158, ADR 0184)

A `store Map[K, V]` roots three key-aware queries. `values` is the default lift a
bare map query already used; `keys` and `entries` expose what it discarded:

- `map.keys -> Query[K]` — the keys
- `map.values -> Query[V]` — the values
- `map.entries -> Query[MapEntry[K, V]]` — each entry as a nominal record `{ key: K, value: V }`, read with `.key`/`.value`

`MapEntry[K, V]` is a *named* record, not a tuple — bynk stays nominal (ADR 0120),
so an entry rejoins its key and value without an anonymous pair. The whole
single-argument builder/terminal vocabulary applies to `.entries` unchanged; a
read handler projects each entry into a named type (`entries.map(e => Row { id:
e.key, … })`) so the stored value need not carry a denormalised copy of its key.
`MapEntry` is non-boundary (ADR 0183), so that projection is what a handler
returns across a boundary, never the raw entry.

### Terminal vocabulary

Terminals execute the query. On a storage query they return `Effect[T]`; on an in-memory collection they return `T` directly:

- `collect` — materialise the full result as `List[T]`
- `first` — first element as `Option[T]`
- `firstOrElse(default: T)` — first or fallback
- `count` — count without materialising
- `fold(init: U, f: (U, T) -> U)` — reduce
- `sum`, `min`, `max`, `average` — numeric and ordered aggregates as specialised terminals
- `any(p: T -> Bool)` — existence check, short-circuits at the first match
- `all(p: T -> Bool)` — universal check, short-circuits at the first counter-example
- `forEach(f: T -> Effect[Unit])` — effectful iteration with no collected result

`forEach` is the streaming-shaped terminal — useful for processing large result sets without materialising the whole thing. True asynchronous streaming iterators are deferred (see below); pagination via `take` + `skip` or cursor-based with `sortBy` + filter on a strict-greater predicate covers the common cases.

### Time-window builders on Log

`Log[T]` carries an implicit timestamp on each entry. Queries on logs get time-shaped builders that compose with the general vocabulary:

- `log.since(t: Time)` — entries appended at or after `t`
- `log.before(t: Time)` — entries appended before `t`
- `log.between(start: Time, end: Time)` — closed range
- `log.recent(n: Int)` — last `n` entries, newest first
- `log.reversed` — reverse iteration order

These compose with the general builders:

```
events.since(yesterday)
      .filter(e => e.kind == Order)
      .map(e => e.payload)
      .collect
```

For `Log`, the time index is implicit and always present. `since`, `before`, `between`, and `recent` always use it.

### Indexing

Refinement annotations on storage declarations express access patterns:

```
store reservations: Map[ReservationId, Reservation] 
    @indexed(by: expiresAt, by: orderId)
```

This tells the runtime to maintain secondary indexes on `expiresAt` and `orderId`. The compiler analyses query expressions and routes them through indexes where the predicates match — `filter(r => r.orderId == oid)` becomes an index lookup rather than a scan.

The runtime and compiler split responsibilities:

- The **runtime** maintains the indexes, transparently updating them when the underlying map is written to. Index maintenance is part of the agent's atomic-commit machinery; an indexed map is no less atomic than an unindexed one.
- The **compiler** routes queries to indexes during analysis. When a query could use an existing index, it does. When a query would scan but a matching index could be declared, the compiler emits a warning naming the missing index and suggesting the annotation. When an index is declared but no query uses it, the compiler emits a warning noting the unused index. When a query is ambiguous (multiple indexes could match), the compiler picks the most selective and notes the choice in the build report.

This makes index hygiene a build-time concern rather than a production-incident concern. The developer writes natural queries; the compiler ensures their indexes match. This is where the language pays off most directly for the application developer — the cost of index management is moved from runtime debugging to build-time review.

For `Map`, the primary key is always indexed; secondary indexes are opt-in. For `Set`, membership is always indexed. For `Log`, the time index is always present.

### Cross-shape queries

The join combinators work across storage shapes within the same agent. A common pattern is joining a `Map` with a `Log`:

```
recentReservationEvents
  .since(now - 1.hour)
  .joinOn(Reservations.filter(r => r.status == Pending),
          left:  e => e.reservationId,
          right: r => r.id)
  .map((event, reservation) => ReservationAuditEntry { ... })
  .collect
```

Indexing applies across shapes — the join uses the Log's time index for the `since` filter and the Map's primary key for the lookup side. The compiler's query analysis handles cross-shape queries the same way it handles intra-shape ones.

### In-memory collection iteration with effects

In-memory `List[A]` carries the same builder vocabulary as queries plus four effectful iteration methods for cross-agent fan-out and Result-handling. The defaults are tuned to the common case:

- `List[A].traverse(f: A -> Effect[B]) -> Effect[List[B]]` — sequential, awaits each `f(a)` before the next; returns results in input order. When `B` is plain (non-Result), this collects the values directly.
- `List[A].traverse(f: A -> Effect[Result[B, E]]) -> Effect[Result[List[B], E]]` — sequential, **short-circuits on the first `Err`**. The same name covers the Result-returning case because short-circuit is the overwhelmingly common need (validation pipelines, sequential checks, anything where one failure invalidates the rest). The compiler dispatches on the function's return type.
- `List[A].traverseAll(f: A -> Effect[Result[B, E]]) -> Effect[List[Result[B, E]]]` — sequential, collect-all. Returns the full list of outcomes without short-circuiting. Used when failures are domain-level information the caller wants to gather (form validation with all errors, bulk processing where partial success matters, compensation tracking).
- `List[A].parTraverse(f: A -> Effect[B]) -> Effect[List[B]]` and `List[A].parTraverse(f: A -> Effect[Result[B, E]]) -> Effect[Result[List[B], E]]` — concurrent counterpart of `traverse`. Issues all calls in parallel, awaits the slowest, returns in input order. For Result-returning functions, short-circuits on the first `Err` once all in-flight calls have completed (cannot cancel calls already issued, but does not start new ones after an Err is observed). For independent receivers this is genuinely concurrent; for shared receivers the runtime's per-pair FIFO degrades it gracefully to sequential processing at the receiver.
- `List[A].parTraverseAll(f: A -> Effect[Result[B, E]]) -> Effect[List[Result[B, E]]]` — concurrent collect-all.

The naming follows established FP convention (Cats Effect, fs2, ZIO all use `parTraverse`). The shape: short-circuit is the default because it's the common case; `traverseAll`/`parTraverseAll` is the explicit collect-all variant. There is no opaque suffix; each operation's role is in its name.

These are eager: applied immediately to in-memory lists, not lazy. They do not appear on `Query[T]` — collected results have already left the lazy domain by the time effectful iteration begins.

### Effect tracking

Queries declare reads as part of the storage capability that comes with the agent's storage fields. They do not introduce additional capabilities. A handler that reads from its own storage has the storage-read effect implicitly via the field declarations; queries against those fields fold into the same effect, no additional annotation required.

Pure functions that construct `Query[T]` values without executing them have no effects. The lazy/eager split lines up with the pure/effectful split: building is pure; terminating against storage is effectful.

### What is deferred

- *Cost-based query optimisation* — beyond index selection, no plan reordering or cost models. The compiler picks indexes; the developer picks the query shape. Sufficient for the target workloads; cost-based optimisation can be added if pressure emerges.
- *Materialised views* — derived storage maintained automatically as upstream sources change. Useful but substantial; out of v1.
- *Reactive queries* — subscribing to a query and getting notified when results change. Closer to the events system; would be a future composition.
- *Streaming asynchronous iterators* — true async streams beyond `forEach`. Pagination via `take`/`skip` and cursor patterns covers the common cases.
- *Time-travel queries* — point-in-time queries against `Log` to reconstruct historical state. The data is there; the language does not yet expose a primitive.
- *SQL-like declarative syntax* — combinator chains are the only form. LINQ proved that even with both options method syntax dominates.

## 12. Consistency Model

Layered, with the natural guarantee at each scope matching the intuition appropriate to that scope.

| Scope                                       | Guarantee                                             | Source                                       |
|---------------------------------------------|-------------------------------------------------------|----------------------------------------------|
| Within a single handler                     | Serialisable, read-your-writes, atomic commit         | Input gate / single-threaded execution       |
| Across handlers in the same agent           | Serialisable                                          | Single-threaded execution                    |
| Direct cross-agent interactions             | Read-your-writes for the sender                       | Per-sender FIFO                              |
| Transitive cross-agent interactions         | Eventual                                              | Network, asynchronous delivery               |
| Explicit ordering across transitive chains  | Read-your-writes                                      | Programmer's explicit awaits                 |

Consistency is a first-class parameter of query execution. The default within a handler is the atomic snapshot; `collect(eventual)` opts into a cacheable, replicable read; `collect(asOf(t))` requests a point-in-time read where the runtime supports it.

**Delivery semantics** complement the consistency model. Cross-agent commands and events have at-least-once delivery: outbound messages release only when the sending handler commits, so an aborted handler produces no effects, but a committed handler's outbound calls may be delivered more than once if the runtime cannot confirm initial processing. WebSocket sends to held connections are at-most-once: lost frames are not retried, and the runtime reports failure asynchronously via the `on close` handler rather than at the call site. Applications that need stronger guarantees over a WebSocket layer their own acknowledgement protocol on top, the way Phoenix Channels or Pusher do in practice. The guarantee the language commits to is the *floor*; the runtime determines what happens above it, and the program decides what guarantees it actually needs.

### Handler-level idempotency via the Idempotency capability

At-least-once delivery is made safe by construction when receivers are idempotent. Bynk handles this through the **`Idempotency` capability**: a handler that needs deduplication declares `given Idempotency` and calls `Idempotency.dedup(...)` with a key and retention window. On a subsequent invocation with the same key inside the retention window, the call short-circuits the handler — the cached outcome is returned without re-executing the remaining body.

```
on reserve(qty: Int, orderId: OrderId) -> ReserveOutcome
    given Clock, Idempotency {
  <- Idempotency.dedup(on: orderId, expiresAfter: 24h)?
  ...
}
```

The `Idempotency.dedup` call declares two things to the runtime: the deduplication key (any expression in scope at the call site) and how long the dedup record is retained. The provider records the call's eventual outcome atomically with the handler's other commits. On a subsequent invocation with the same key, the call returns the cached outcome without re-executing the rest of the handler body. After expiry, the record is collected; the same key would re-execute.

The dedup record is written atomically with the handler's other commits. If the handler completes — Ok or Err — the result is cached. If the handler aborts via fault, no record is written, and a retry re-executes the body. This extends the *handler is the atomic unit* promise to the dedup mechanism itself: the programmer doesn't have to reason about partial states between executing the body and recording the dedup.

For event subscribers, the canonical key is the event envelope's identifier:

```
on customer.payment.PaymentConfirmed(e: PaymentConfirmed)
    given Idempotency {
  <- Idempotency.dedup(on: e.eventId, expiresAfter: 7d)?
  ...
}
```

The `eventId` is always populated by the runtime's event envelope (Section 7's Events protocol). Event subscribers that take effects on receipt should default to this pattern; subscribers that only read or transform pure state are trivially idempotent and need no dedup call.

**What the capability does and doesn't guarantee.** It makes the handler **mechanically idempotent**: the provider won't re-execute on a duplicate key. But the body must also be **semantically idempotent**: if it issues a side-effecting cross-agent call mid-body, that call's receiver also needs its own idempotency contract — via its own `Idempotency.dedup` call, an idempotency key in its protocol, or a naturally idempotent operation. The architecture is consistent: every agent's handlers are responsible for their own idempotency contract, and the capability gives every handler the same mechanism to express it.

For operations that genuinely cannot be made idempotent at the receiver — issuing an external charge, sending a notification — the caller supplies a deterministic identifier derived from its own context, and the receiver dedupes against it. The `dedup` call accepts any expression as the key, so a `PaymentGateway.charge(amount, customer, idempotencyKey)` flows cleanly through the same mechanism whether the receiver is in the same system or external.

**Choosing the retention window.** Too short and legitimate retries miss the cache and re-execute. Too long and the dedup table grows unbounded. The right window is a few times the worst-case end-to-end retry interval of the protocol involved — for HTTP retries through a few hops, hours to a day; for queue-driven retries with backoff, often longer. The explicit `expiresAfter:` parameter makes this a visible, per-call decision rather than an invisible runtime default.

**Idempotency provider variants.** The capability has multiple providers, the same way Sagas does: in-memory (handler-local dedup, lost on restart, right for short-window cases) and durable (records survive crashes, right for the canonical at-least-once safety story). The handler shape is the same under both; the provider determines the durability semantics.

## 13. Failure Model

The language distinguishes two genuinely different kinds of failure and refuses to conflate them.

- **Outcomes** are typed values that are part of a handler's contract. They name the anticipated domain-level results that callers must handle: the user wasn't found, the inventory is depleted, the payment was declined, the text was too long. Outcomes appear in handler return types as `Result[T, E]` or as richer custom sum types, and the caller is required by the type system to pattern-match against each branch. They flow through messages, participate in tests by direct invocation, and belong squarely to the domain.
- **Faults** are unrecoverable runtime events that mean a handler has lost the plot — storage I/O failure, runtime exhaustion, programmer assertion violation, network partition the platform cannot mask, unrecoverable bugs. Faults are not values, are not returned, are not part of any function's signature. They abort the handler atomically (no storage writes commit, no outbound messages release) and propagate to the caller as a runtime-level "the call failed" signal rather than a typed outcome.

Most languages collapse these into a single hierarchy of "errors" or "exceptions" with a rich inheritance tree open to user extension. This is convenient in the small but a foot-gun in the large: the same construct is used both for things developers expect (and should handle in the program) and things they don't (and should let infrastructure handle), and nothing in the type system reminds them which is which. The result, in practice, is silent corruption when expected failures are caught generically and unrecoverable cascades when unexpected failures are caught and ignored. Naming the two kinds differently in the language solves this at the source: outcomes appear in signatures and must be matched; faults don't appear and are propagated to where they can be handled correctly.

The atomic-handler semantics committed to elsewhere make the fault story particularly clean. Each handler is a transaction: storage writes commit at the end, outbound messages release at the end, both or neither. A fault during execution aborts the entire transaction. The Cloudflare runtime provides this via input and output gates; the language presents it as a guarantee that a handler either completed successfully and produced its outcome, or did not and produced nothing.

The same atomicity resolves cross-agent failure with no extra machinery. When agent A calls agent B and B faults mid-handler, B's transaction aborts, no state in B changed, no further messages from B were sent, and A's `<-` bind on the call sees a fault rather than an outcome. A's default is to propagate the fault upward. Converting a fault into an outcome is deliberate and visible:

```
on read(key: Key) -> Result[Data, Unavailable] {
  attempt {
    let bytes <- Store.read(key)
    Ok(bytes |> decode)
  } recover {
    Err(Unavailable)
  }
}
```

The `attempt`/`recover` form is the only construct for converting a fault into an outcome. It is deliberately verbose — doing it should be a design decision, not a habit — and most handlers should not use it. The platform's restart semantics handle most faults better than catch-and-retry inside the program would.

Handler signatures express the contract directly:

```
-- typed outcome; caller must handle both branches
on debit(amount: Money) -> Result[Balance, InsufficientFunds] { ... }

-- bare value; no domain failure mode (any failure is by definition a fault)
on currentBalance() -> Money { ... }

-- richer outcome space than Result allows
on tryDispatch(req: Request) -> DispatchOutcome { ... }
  where type DispatchOutcome = Dispatched(Receipt)
                             | Queued(QueuePosition)
                             | RateLimited(Duration)
                             | Rejected(Reason)
```

A handler returning bare `Money` is making a contract claim: *no expected domain failure exists for this call*. Any failure that does occur is, by definition, a fault.

### Compensation via the Sagas capability

The atomic-handler invariant covers what a single agent commits. It does not cover multi-agent flows: a sequence of cross-agent calls where each remote effect commits as it succeeds, and a later step's failure leaves earlier effects standing. The architecture has structurally ruled out the alternative — distributed atomic commit needs coordination protocols that the target platforms typically don't expose, and that would compromise availability anyway. So when a flow spans agents and can fail partway through, the only honest answer is explicit compensation: a way to undo earlier steps with new actions, since you cannot roll them back.

Bynk handles this through the **`Sagas` capability** (Section 18). A handler that needs to register compensating actions declares `given Sagas` and calls `Sagas.compensate(...)` with the action to run if the handler aborts. Registered actions run in LIFO order on abnormal exit (via `?` propagating Err, or via fault propagation), each wrapped in best-effort attempt by the provider. On normal exit, registered actions are discarded.

```
on place(u: UserId, c: Cart) -> Result[Receipt, OrderError]
    given Inventories: Ref[commerce.inventory.Inventory], 
          Payments:    PaymentGateway,
          Fulfilments: Ref[commerce.fulfilment.Fulfilment],
          Sagas {

  validateCart(c)?
  user := Some(u)
  cart := Some(c)

  let reservations <- reserveAll(c.items)?
  <- Sagas.compensate(() => 
       reservations.parTraverse((sku, rid) => Inventories(sku).release(rid)))?

  let authId <- Payments.authorise(c.total, u).mapErr(PaymentDeclined)?
  <- Sagas.compensate(() => Payments.refund(authId))?

  let shipId    = ShipmentId.forOrder(id)
  let lineItems = c.items.map(i => LineItem { sku: i.sku, qty: i.qty })
  <- Fulfilments(shipId).schedule(lineItems, u).mapErr(FulfilmentUnavailable)?
  <- Sagas.compensate(() => Fulfilments(shipId).cancel())?

  status := Placed
  Ok(Receipt { orderId: id, total: c.total })
}
```

Each `<-` is an awaited cross-agent or capability call; its remote effect commits before the call returns. Each `Sagas.compensate(...)` registers a deferred action that closes over the bindings in scope at registration. If any `?` propagates Err, the handler exits with that Err: the Sagas provider's abort hook runs through registered compensations in LIFO order, each awaited as it executes, and the Err propagates to the caller. If a fault occurs, the same LIFO unwinding happens before the fault propagates. The handler's atomic transaction is independent of whether compensations run — compensations execute as awaited capability calls during the unwind, their remote effects committing as they go, regardless of whether the local handler later commits.

The `given Sagas` declaration makes the compensation dependency visible at the handler signature. A reader scanning the handler can see — without descending into the body — that this handler does saga-style compensation. The reader can also identify exactly which Sagas provider is in scope (in-memory or durable) by following the resolution from the handler's environment.

**Compensations require idempotent targets.** `Inventory.release(reserveId)` must produce the same result on repeated calls. This is the same requirement at-least-once delivery already imposes; the explicit `Sagas.compensate` call makes the requirement visible at the call site. A handler that targets a non-idempotent operation in a compensation is a latent bug; the explicit call gives the reviewer somewhere to look.

**Composition with `attempt`/`recover`.** When the user wants to catch faults and convert them to outcomes, `attempt` wraps a block. Saga registrations inside the block behave the same as anywhere else — they run on abnormal exit, before the fault reaches the `recover` clause:

```
attempt {
  let r <- step()
  <- Sagas.compensate(() => undo(r))?
  doSomething()
} recover fault {
  -- the compensation for r has already run by this point
  Err(SomeFault(fault.info))
}
```

The `attempt` block is just another block; the Sagas provider's abort hook fires on abnormal exit regardless of whether the abort propagates further or is caught by `recover`.

**Sagas provider variants.** The `Sagas` capability has multiple providers (Section 18):

- *In-memory* (the default for most handlers): registrations live in handler-local state, compensations run on abort within the same handler invocation. Lost on agent runtime crash. Right for the common case.
- *Durable*: registrations persist via the platform's storage; compensations survive crashes; explicit forward/undo step pairs; recovery via the framework's runtime. Right for long-lived workflows where partial-progress recovery matters.

The handler is the same shape under both; the provider determines the durability semantics. A handler can switch from in-memory to durable Sagas without code changes — only the provider binding changes.

**What the capability covers and what it doesn't.** `Sagas.compensate` handles the common shape: register a compensation against an awaited step, run it on abort. Several patterns are not directly handled and remain library- or agent-level:

- *Long-running flows across handler boundaries.* A flow that spans days, with steps depending on external events arriving later, does not fit a single handler. **The saga is just an agent**: its state captures progress, its handlers advance the state, compensations are explicit calls, recovery is the agent's resumption from durable state. The language already provides everything needed; saga-as-agent uses existing primitives.
- *Retry policies.* The handler aborts on Err; retry-with-backoff is achieved via a wrapper provider on the capability being retried (e.g., `Payments.withRetries(...)` at the `provides` declaration). Saga compensation operates on the outcome after retries have been exhausted.

What is deliberately deferred:

- **Declarative supervision** beyond what the platform provides. The runtime's restart semantics cover most faults. Declarative relationships (e.g. "if this agent faults repeatedly, escalate to that one") are a future addition rather than part of the floor.
- **Sugar for outcome chaining.** `Result`-typed sequences become verbose under deep matching. A `?` propagator (Rust-style), do-notation, or block forms are desirable but are sugar over the foundation rather than part of it.

The pedagogical content is direct: the outcome/fault distinction is something experienced engineers learn through production scars, where mixing the two has led to either silent corruption or unrecoverable cascades. The language teaches it by making it the only way to express failure at all — students write `Result[T, E]` for the things they can describe, and they learn that the things they cannot describe are faults, which is a sharper way to think about reliability than "what exceptions might this throw?"

## 14. Validation

The architectural cleanness of the language gives validation a particularly tractable shape. Typed capabilities for all effects, atomic handlers, bounded-context boundaries, and opaque-type construction limited to owning contexts each contribute to a code base that is testable by construction. This section names the language's commitments around testing and invariant-checking.

### One predicate surface

The organising idea is that **testing is not a separate sublanguage**. The language already has a way to state a checked claim — an agent `invariant` is a pure `Bool` predicate (`implies`, `is`, the operators, pure methods) quantified over an agent's reachable states and enforced at the commit boundary. Every other kind of check reuses *that same predicate*. There is no second assertion grammar and no matcher library: `expect r is Ok(_)` in a test is the same `is` an invariant uses.

One predicate surface runs over a **ladder of subjects** — a value, a refinement domain, a function call, a committed-state snapshot, the step between two states, and a whole history of steps — each subject *supplied* (a worked example) or *generated* (property-style); *checked* at one of three moments (the commit boundary, the dev/test call site, or the test runner); and *run* at one of three tiers (`unit`, `integration`, `system`). The rest of this section walks that surface. This generalises the invariant↔test duality the architecture already had — *invariants are the contract half of validation; tests are the behaviour half* — into a single spine, and it is the theme the testing track built out slice by slice.

### The architectural foundation

Several properties make this language testable by construction.

*Capabilities are the substitution seam.* Every effect is a typed capability declared via `given`. A test substitutes a single method's provision at that seam with `provides Cap.method(pattern) returns <value> | fails` — a value or an injected fault, never a re-implementation — or promotes the tier so the real collaborator runs. The mocking problem in conventional languages collapses into supplying (or withholding) a value at a typed seam; the word "mock" is gone from the language.

*Handlers are deterministic units.* A handler is a function of `(state, message, capabilities) → (new state, outputs, return)`. With its seams supplied, calling a handler is unit-testable with no infrastructure to stand up.

*Bounded-context boundaries are the tier boundary.* Tests outside the owning context reach it only through the public surface (services and exported agent contracts). This gives a natural unit-versus-integration-versus-system split — the *same* test body run against stubbed seams (`unit`), against real collaborators within one context (`integration`), or against contexts stood up as the Workers they deploy as, across the real serialisation wire (`system`) — one body promoted, not three kinds of test.

*Effects at the seam are observable.* Because a capability is injected at a known seam, the test build records its calls automatically. A test asserts over that interaction — `expect Cap.op called once with …`, or the `trace(Cap.op)` list — with no setup, and the recording is stripped from the deploy build.

### Suites, cases, and expectations

Testing happens in **suites**. A `suite` names the unit it targets and holds named `case`s — and test-ness is a property of the `suite` *declaration*, not of a file's name or path. A `suite` is legal in any file and is stripped from the deployable build (never type-checked for production, never emitted), while the test runner compiles and runs it; discovery walks the whole source tree for `suite` declarations rather than a designated folder. A `.bynk` file may therefore carry a `context` and its `suite` together — the shareable, single-file, playground-friendly shape.

```
suite commerce.orders {
  case "placement succeeds with a valid order" {
    let cart = Cart {
      items: [CartItem { sku: Val[Sku], qty: Quantity.of(3)?, unitPrice: Money.of(1000, gbp) }],
      total: Money { minorUnits: 3000, currency: gbp },
    }

    provides Inventory.reserve(_)  returns Reserved(Val[ReservationId])
    provides Payment.authorise(_)  returns Val[AuthId]

    let result <- Order.place(Val[UserId], cart)

    expect result is Ok(_)
    expect result.unwrap().total == cart.total
    expect Inventory.reserve called once
    expect Payment.authorise called once
  }
}
```

Several commitments are baked into this shape.

*A suite is stripped from the build, not a distinct context kind.* There is no third top-level declaration kind for tests. A `suite` names its target and is legal anywhere; the build (`bynkc compile`) skips every `suite`, the runner (`bynkc test`) compiles and runs them, and a reader tells production from test by the `suite` keyword — not by a file path or a `.test.bynk` marker.

*`expect` is the one assertion.* `expect <bool>` checks a predicate — the same pure-`Bool` surface as an `invariant`, an `ensures`, or a `transition` — and is valid only inside a `case`. On a top-level comparison (`==`, `<`, …) a failure reports expected-versus-actual operands, not just a location. There is no separate matcher vocabulary; `expect result is Ok(receipt) && receipt.total == expected` matches the variant, binds `receipt`, and asserts on the binding in one expression, exactly as an invariant predicate does — one `is`, used consistently.

*Tiers are a dial, not a taxonomy.* A `case` runs `as unit | integration | system`; `unit` is the default and is never written, and `as` on the `suite` sets a default each `case` may override (case wins). The body is byte-for-byte identical across tiers — only how much of the real world runs changes. Participants are *inferred* from the unit's transitive `consumes` graph (there is no wiring clause), and a `system` case stands its contexts up as the real Workers they deploy as, exercising the serialise → JSON → deserialise edge the in-process tiers never touch. A `property` generates rather than promotes, so it carries no tier.

*`provides` supplies one seam.* A `provides Cap.method(<argument pattern>) returns <value> | fails` clause overrides a single collaborator method — an argument pattern on the left (the one predicate surface: a literal, `_`, or an `is` narrowing; first matching clause wins) and a *value* or `fails` on the right, never a computed body. A double that needs logic is the signal to promote the tier. A sequenced `returns each [<outcome>, …]` supplies one outcome per successive call, the last repeating. `provides` targets capability seams only — an agent's realness is the tier's job, not a provider's — and precedence runs case `provides` > suite `provides` > the tier default.

*`Val[T]` fabricates a valid inhabitant.* `Val[T]` draws a value from `T`'s refinement domain (refinement-respecting; a sum yields its first variant, a record fabricates each field, an opaque type wraps its base zero), and `Val[T](pin)` pins a specific one, refinement-checked at compile time. It is admitted only in test scope. A `String where Matches(…)` has no generator and must be pinned. `Val[T]` replaced the earlier `Mock[T]` — a straight rename of the value fabricator — and with it the re-implementation `mocks` block was retired in favour of per-seam `provides`, removing the word "mock" from the language entirely.

*Interaction is observed automatically.* Because capabilities are injected at a known seam, the test build records their calls with no `provides` or setup. `expect Cap.op called` / `never called` / `called once` / `called <n> times` / `called with <pred>` / `A.op before B.op` assert over that record; `trace(Cap.op)` yields the recorded calls as an ordinary `List` for anything the sugar does not cover. Recording is emitted only under the test build, so production carries no cost.

What a suite is *not*: a separate language for tests, a hidden runtime, or a way to bypass type checking. A `case` calls into the architecture with controlled seams and asserts about results; the architectural rules — typed handlers, atomic commits, refinement validation, invariant checking — apply unchanged.

### Invariants and transitions on agents

Agents carry their own predicates, checked at every commit for free — the contract half of the same surface, requiring no test to run. An **invariant** is a universally-quantified property that must hold at every observable state; a **transition** widens the subject from a single state to the *step* between two.

```
agent Order(id: OrderId) {
  store status:     Cell[OrderStatus]    = Pending
  store paymentRef: Cell[Option[AuthId]] = None
  store cart:       Cell[Option[Cart]]   = None

  invariant paid_has_payment_ref:
    status == Paid implies paymentRef.isSome()
  invariant cart_set_once_active:
    status != Pending implies cart.isSome()

  -- handlers ...
}
```

Invariants serve simultaneously as domain documentation, runtime validation, and test material. The runtime enforces them at each handler commit boundary: a handler that would commit a state violating an invariant fails as a fault, the commit aborts, the agent reverts. This composes with the failure model already in place.

The architectural fit is strong. Invariants are agent-shaped — they belong to the same primitive that already holds state. They are the direct language-level expression of what DDD calls an aggregate's *invariants*, which is what the architecture is already trying to enforce. They compose with the failure model: violations are faults, not outcomes. They reduce test burden — a property guaranteed by an invariant doesn't need test cases verifying it.

**The predicate language.** Invariant predicates are ordinary expressions with three small additions. `implies` is a logical-implication keyword (`P implies Q` reads as P → Q, equivalent to `!P || Q` but directional and prose-readable). `is` is pattern-matching as an expression (`expr is Pattern` evaluates to a Boolean, optionally binding captures that remain in scope after a successful match). Storage cells are implicitly dereferenced in expression position: `status == Paid` rather than `status.read() == Paid`. Cells are not first-class values — they cannot be passed as arguments, only read or assigned — so position disambiguates: expression context reads, `name := expr` context writes. The implicit-read rule applies to handlers as well, not only invariants, and it touches only `Cell`; `Map`, `Log`, `Queue`, and `Cache` accesses remain method-shaped because they are inherently structured.

**Checking time.** All invariants are runtime-checked at handler commit. The handler runs its logic to completion, builds the proposed committed state, and the runtime evaluates each declared invariant before accepting the commit. If any fails, the commit aborts as a fault and the agent reverts to its pre-handler state. Intermediate states within a handler are *not* constrained — a handler may briefly violate an invariant while transitioning between consistent states, as long as the committed state satisfies it. This matches how transactional databases handle constraints (deferred to commit) and lets handlers be written naturally.

On top of runtime checking, the compiler performs control-flow analysis and flags **statically-provable violations** as errors. A handler whose every path provably commits a state violating an invariant is a compile-time error, not a runtime failure. This is the free win: simple violations get caught early without committing to full static verification. Static *satisfaction* checking (proving an invariant always holds rather than just that it never provably fails) remains open as a future enhancement.

**Per-agent scope.** Invariants constrain a single agent's reachable states. System-level properties (the sum of reservations across all Inventories equals the original stock) are eventually-consistent and enforceable only through compensation, not through atomic commit. The language doesn't try to make these first-class; library-level scenarios, saga compensation, and external monitoring are the right tools. If a property genuinely spans agents, this is a signal either to fold them into one agent (because they share invariants), or to express the property via the saga machinery.

**Transitions widen the subject to the step.** Beside its snapshot invariants, an agent may declare a `transition` — a predicate over the `old` committed state and the `new` state a commit would persist (`old.status is Paid implies new.status is Paid`). It is the same predicate surface; `old`/`new` are contextual (special only inside a `transition`), and it is checked at the commit boundary from the second commit onward — the genesis commit has no `old` and is skipped. Like an invariant it is carried by the agent and inherited by every test for free, at every tier. It is deliberately *not* attacked by the runner with fabricated states: a valid agent state need not be reachable, so behavioural generation over transitions is a driven-handler-sequence concern (the history rung below), not value fabrication.

### Generated subjects: properties, contracts, and histories

Supplied examples are one half of the surface; the other is **generation** — and it is first-class in the language, not a library add-on. Where a `case` supplies its subjects, these constructs generate them, seeded and reproducible, shrinking any failure to a minimal counterexample with a copy-paste reproduce line.

*Properties generate values.* A `property` is the generative sibling of `case`, legal in the same `suite`: `for all x: T` binds `x` to a *generated* inhabitant of `T` drawn from its refinement domain (boundary-inclusive), an optional `where <pred>` filters the generated tuples, and the body is one or more `expect`s over the same predicate surface. A property earns its keep when a claim should hold across a *range* — a relationship between inputs and an output, monotonicity, a round-trip — where one named `case` cannot; a property that merely restates a type's own refinement proves nothing and is flagged.

*Contracts are a function's invariants.* A pure `fn` may carry named `requires` (preconditions over its parameters) and `ensures` (postconditions over the parameters and `result`) between its return type and body — again the same predicate surface. A contract is checked at two points for free: a dev/test call-site guard on every call (stripped from the deploy build, so no production cost or behaviour change), and a runner *attack* that generates arguments over the parameter domains, filters them by the `requires` (exactly as a `for all … where` does), calls the function, and checks the `ensures`. A claim about one result belongs in `ensures` — checked everywhere, generated for free; a `property` is for claims that are relational or span calls.

*Histories generate whole behaviours.* The top of the ladder: `for all run: History[Agent]` generates a bounded, seeded *sequence* of an agent's handler calls, **drives them through the real handlers** from the agent's initial state, and binds `run` — an ordinary `List` of steps — for the predicate to judge. Each step carries which handler ran (`.call`, a sum matched with `is`), whether it committed (`.accepted`), and the committed `.old`/`.new` state pair. Because behaviour is generated by *driving sequences, not fabricating states*, every state in the run is one a handler actually reached — the reachability concern that keeps `transition` off the runner is answered by construction here. Assertions reuse the `List` surface: "always" is `run.all(...)`, "eventually" `run.any(...)`, "P before Q" a prefix check via `run.upTo(step)`. There is no temporal vocabulary — a history is a runner *sample* over bounded runs, so unbounded liveness is deliberately inexpressible. Single-agent histories are the shipped surface; multi-agent protocols are a named follow-on.

Property-based testing was once expected to sit in a library on the QuickCheck model; instead it became a first-class part of this surface, because the typed-message machinery makes generation mechanical and handler determinism makes it reproducible. Cross-agent *scenarios* — scripted, actor-driven flows with assertions across several agents — remain a library-level and `system`-tier concern that consume existing primitives without new language surface.

### Together

Supplied `case`s describe behaviour existentially (*there exists a case where this works*); properties, contracts, and histories describe it over generated ranges; invariants and transitions describe contracts universally (*for all reachable states and steps, this holds*). All of them are the same predicate over one ladder of subjects, bound to the same handler-and-state machinery — which is what makes the surface cohere rather than accumulate. A reader of an agent sees examples of its behaviour, claims about its behaviour, and the contracts its states and steps must keep, in one vocabulary.

## 15. Type System

The type system is the implementation of the architectural commitments made elsewhere in this document. Opaque types implement the visibility model (Section 8); capability interfaces implement the effects mechanism (Section 5); generic types implement storage and held resources (Section 10); closed sums implement outcomes (Section 13); constrained refinement implements domain type validation (at type declarations), event subscription patterns (Section 7), agent invariants (Section 14), and capability operation contracts (Section 18). The shape that follows is largely determined by what has come before; there are fewer free parameters than first appears.

### Core

Standard Hindley-Milner with closed sums and nominal records. Parametric polymorphism with principal types. Inference inside bodies — local `let`s, generic instantiation, and lambdas checked against an expected function type — with annotations required on every named declaration: a `fn` states its parameter and return types, and a lambda with no expected type annotates its parameters. Generic types (`Cell[T]`, `List[T]`, `Map[K, V]`, `Result[T, E]`, `Connection[F]`) and generic user types. No subtyping. Effects are declared, not inferred, on everything with a name: a declaration's `Effect[T]` return and its `given` capabilities are written and checked. The one inference is local — an unannotated lambda's effect is read off its body, so a lambda that binds with `<-` has type `(…) -> Effect[T]` — and it changes no named signature (type-system spec §2.8.4).

*Nominal records, not structural.* A type's name carries meaning, so two records with the same fields and different names are distinct types. A `Cart` is a `Cart`, not "any record with these fields." Cross-context boundaries become explicit at the type level — a context that wants to accept "anything cart-shaped" declares the type it accepts.

*Closed sums, not polymorphic variants.* Variants are fixed at the type's declaration. Pattern matching is exhaustive by construction; adding an outcome variant is a deliberate change visible in the type. Polymorphic variants are more flexible but obscure error messages.

*No subtyping.* All polymorphism is parametric. Opaque types and capability interfaces look subtype-like but are not — they are abstract types and named interfaces, handled by HM with extensions rather than by a subtype lattice. The absence of subtyping keeps inference decidable, error messages mechanical, and the type system predictable.

### Annotation policy

Required:

- Function and handler declarations (parameters and returns)
- Agent storage declarations
- Cross-context type references
- Capability sets via `given`

Inferred:

- Local `let` bindings
- Anonymous functions in unambiguous contexts (passed to `map`, `parTraverse`, `collect`, etc.)
- Generic instantiation where unique

The rule: things that are part of the architecture's contracts are annotated; things that are internal scaffolding are inferred. Visible boundaries, invisible internals. The policy matches what the architecture already does at the bounded-context and service levels — services declare their boundary, agents declare their state, contexts declare their imports and exports.

### Opaque types

A context can declare opaque types whose internal representation is hidden from other contexts. Inside the context, an opaque type is whatever its definition makes it; outside, it is an abstract type — held, passed, compared for equality, but not constructed or destructured except through exported operations.

```
context commerce.order {
  type OrderId      -- representation hidden externally
  
  fn OrderId.fresh() -> OrderId given Random { ... }
  fn OrderId.parse(s: Text) -> Result[OrderId, ParseError] { ... }
  
  exports opaque { OrderId }
}
```

This is the ML-module / Haskell-abstract-data-type mechanism. The visibility flags from Section 8 (opaque, transparent, private) are applied at the export clause, not at the type declaration. The type itself is declared once; visibility is declared at export.

A consequence: transparent types are still nominally distinct from their structural representation. `type Money = { minorUnits: Int, currency: CurrencyCode }` is nominally `Money`, not `{ minorUnits: Int, currency: CurrencyCode }`. The structure is visible across the boundary; the name carries meaning.

Distinct from opaque types are *type aliases*: `type Sku = String` makes `Sku` and `String` interchangeable. Aliases are useful for clarity without identity; opaque types are useful when identity itself matters. The syntactic distinction needs to be clear — the provisional shorthand `type T` with no body, exported as opaque, implies a nominal abstract type with compiler-chosen representation.

### Capabilities as typed interfaces

A capability is a named interface — a set of operations with declared signatures.

```
capability Clock {
  now() -> Instant
  sleep(d: Duration)
}

capability PaymentGateway {
  authorise(amount: Money, userId: UserId) -> Result[AuthId, PaymentError]
  refund(authId: AuthId)
}
```

Platform bindings implement these interfaces. A function declaring `given Clock, PaymentGateway` requires both interfaces in scope at the call site. The compiler tracks capability sets through signatures: if `fn foo() given Clock { ... }` and `fn bar() { foo() }`, then `bar` either declares `given Clock` of its own or fails to compile.

Effects, in the broad sense of anything requiring platform support, are visible in signatures by virtue of capability declarations. No separate effect system is needed; the capability mechanism subsumes it.

This is closer to ML modules or Scala's implicit parameters than to Haskell's type classes. Simpler, decidable, and sufficient. The language does not need full type-class machinery (coherence rules, default methods, inheritance hierarchies) — just named interfaces with the substitution discipline of implementations being provided at the platform boundary.

### Constrained refinement

Refinement is permitted at specific architectural points, with **refinement at type declarations as the primary mode**. Putting refinement on a type definition gives the same definition triple duty:

1. *Compile-time type identity.* The refined type is a distinct type the compiler reasons about — not interchangeable with its underlying representation, carrying the constraint as part of its identity.
2. *Runtime validation logic.* The same constraint is the validator. The type's constructor returns `Result[T, ValidationError]`; refined types deserialised from external sources (HTTP bodies, queue messages, event payloads, storage rehydration) are validated against the same predicate the type-checker uses. The refinement is executable as well as checkable.
3. *External schema specification.* The same constraint is the wire-level contract. OpenAPI, AsyncAPI, JSON Schema for storage migrations, and other external schema artifacts are generated from the refined type definitions. One source; many derivations.

This unification — type, validator, schema, from a single declaration — is the architectural payoff. In conventional ecosystems these concerns live in separate tools (the type system, a validation library, a schema generator, a documentation generator) with hand-maintained correspondences and persistent drift. Bynk collapses them.

The refinement points, in priority order:

- *Value type declarations* (the primary point): `type VoucherCode = String where Matches("[A-Z0-9]{8}")`, `type DiscountPercent = Int where InRange(1, 50)`. Define refined types deliberately; reuse the named types everywhere a value of that kind is needed. The constraint is paid for once at the definition and consumed many times.
- *Event subscription patterns* (Section 7): `from Events(PaymentConfirmed { region: Region.Domestic, .. })`.
- *Agent invariants* (Section 14): `invariant: status == Paid implies paymentRef.isSome()`.
- *Actor authorisation invariants* (anticipated, Section 6).
- *Capability operation refinements* (Section 18, framework-internal): capability authors may constrain operations with refinements like `Idempotent on (...)` that consumers can rely on and implementations must satisfy.

**The refinement vocabulary is constrained to data predicates** — propositions about values that can be checked at compile time, executed at runtime, and serialised into external schemas. Initial vocabulary (small, evolving slowly): `Matches(regex)`, `InRange(min, max)`, `MaxLength(N)`, `MinLength(N)`, `Length(N)`, `NonNegative`, `Positive`, `NonEmpty`. Each has precise compiler-implemented semantics; the language commits to a fixed set rather than a user-extensible predicate language. New refinements require language design work — not framework work, not capability work — which keeps the compiler bounded and the validation behaviour consistent.

**Refinement complements primitive choice; it does not substitute for it.** A discipline that compounds with refinement: choose the primitive type that makes the constraint structural rather than asserted. If a complex refinement is needed to compensate for a loose representation, the representation is probably wrong. The canonical example is money: representing it as a fractional number invites a `Scale(2)` predicate to track precision; representing it as an integer count of the minor currency unit (pennies, cents) makes the precision exact by construction and removes the predicate entirely. Refinement is the right tool when the primitive genuinely cannot constrain what the domain requires (`VoucherCode = String where Matches(...)`); it's the wrong tool when a sharper primitive would have made the constraint a non-issue. The two disciplines together — choose the primitive tightly, refine where the primitive doesn't reach — produce types that carry their meaning robustly without the type system growing to accommodate compensation.

What refinement is *not*: refinement is not generally available on function arguments, return types, or arbitrary type expressions at use sites. Use-site refinement was considered and rejected — it produced verbose signatures and shifted refinement from being a property of named types to being a property of every call site. The discipline is: refine to create types worth naming; reuse the named types everywhere. Inline refinement on parameters or returns is permitted in narrow cases (a genuinely one-off constraint that doesn't deserve a named type) but is not the default mode, and tooling should warn when inline refinement looks like it should be a named type instead.

Properties of *functions* — purity, totality, determinism — are inferred by the compiler from a function's surface (no `given` clause, no storage access, exhaustive matching, no calls to known non-deterministic capabilities) rather than annotated. Explicit `where Pure` is reserved for the rare case where a function wants to *commit* to purity as a contract that should produce a compile error if later refactored to break it. This is a separate, smaller concern from data refinement; the two share the `where` keyword but solve different problems.

Pedagogically: students learn refinement as "the way to define domain types that carry their constraints." A `VoucherCode` is a string that matches a specific pattern, by construction; a `DiscountPercent` is an integer in a specific range, by construction. Handler bodies never defensively re-check what the type system has already established; framework boundaries deserialise into validated values or reject the input. The same mental model serves design, validation, and documentation.

### What the language deliberately does not include

*Subtyping*: not present. All polymorphism is parametric.

*Row polymorphism*: not in scope initially. Where you would want "any record with these fields," declare a transparent type for the projection. Architecturally explicit, slightly more verbose.

*Session types*: not in scope initially. Plain agents already model protocol state with handlers transitioning state; session types would catch some protocol violations at compile time rather than runtime, but the temporal dimension they add to the type system is a substantial commitment.

*Higher-kinded types*: not in scope initially. Library-level abstractions can use concrete types. If a `Functor[F[_]]`-style abstraction proves necessary, it can be added later.

*Type class hierarchies*: not in scope initially. Capabilities are flat interfaces; composition is via multiple `given` declarations rather than inheritance.

Each of these is genuinely useful in some contexts, and the language may grow toward them later. The starting position is conservative: include only the type-system features the architecture demonstrably needs.

### Summary

The type system is Hindley-Milner with closed sums, nominal records, opaque types implementing visibility, capability interfaces implementing effects, generic types implementing storage and held resources, and constrained refinement implementing domain validation (at type declarations) and architectural constraints (at event subscriptions, agent invariants, and capability operations). Refinement at type declarations does triple duty — compile-time type identity, runtime validation, external schema generation — which collapses what conventional ecosystems split across separate tools into a single declaration. Annotations are required at contract boundaries and inferred internally. The combinations are well-understood; the novelty is in the specific tuning to a service-tier architectural language and in the unification refinement provides. The result is a type system that students can predict, reviewers can navigate, and the compiler can check decisively — and whose features map one-to-one onto the architectural commitments they support.

## 16. Syntactic Principles

The language commits to a surface syntax that is ligature-friendly (multi-character ASCII sequences that render as single intentional glyphs in editor fonts that support them), parse-tractable (LL(k) for small k), expression-oriented, and prose-readable. Each architectural commitment is visible at the glyph level: a reader scanning a file should recognise what is pure, what is effectful, what holds state, what crosses a boundary, without consulting types.

The source remains ASCII; ligature rendering is an editor concern. The design pressure is to choose two- and three-character sequences that have established rendered forms and whose ASCII shape reads cleanly when ligatures are off.

### Committed glyphs

| ASCII | Rendered | Meaning |
|---|---|---|
| `->` | → | Return type arrow on function and handler signatures |
| `<-` | ← | Await on cross-agent or capability call result |
| `=>` | ⇒ | Lambda body and match-arm body |
| `:=` | ≔ | Storage assignment (set the value of an agent's storage slot) |
| `=`  | =  | Local binding (`let x = expr`) and default initialisation |
| `==` | ≡ | Equality |
| `!=` | ≠ | Inequality |
| `<=` | ≤ | Less-than-or-equal |
| `>=` | ≥ | Greater-than-or-equal |
| `&&` | ∧ | Logical AND |
| `\|\|` | ∨ | Logical OR |
| `!`  | ¬ | Logical NOT |
| `?`  | ? | Outcome short-circuit on `Err` |
| `..` | … | Rest-of-fields in record patterns; range in slices |

These are settled. The rendered column is what they look like in a ligature-aware editor (Fira Code, JetBrains Mono, Cascadia Code, and similar); the ASCII column is what is in the source file and what the parser sees.

### Block, sequence, and expression structure

Blocks are delimited by `{` and `}`. The last expression in a block is the block's value; intermediate statements are separated by newlines. Semicolons are accepted but rare, used only to put multiple expressions on a single line.

The language is expression-oriented: `if`, `match`, and `attempt` are all expressions yielding values; what reads as a statement is simply an expression whose result is discarded or whose result commits a storage change (the case of `:=`).

Generic type parameters use square brackets: `Cell[T]`, `Map[K, V]`, `Ref[Order]`, `Result[T, E]`. Square brackets avoid the grammar tangles that angle brackets create with comparison operators. At the value level generics are usually inferred; explicit instantiation when needed uses the same `[...]` syntax.

Function application, tuples, and grouping all use `(...)`. Member access and module paths use `.` (`order.status`, `commerce.inventory.Inventory`); the compiler disambiguates from context.

### Lexical details

*String interpolation*: `"hello \(name), you ordered \(item.qty) of \(item.sku)"`. The `\(...)` form is unambiguous, composes with normal escapes (`\n`, `\t`, `\\`), and requires no prefix marker on the string.

*Numeric literals*: underscores as readability separators (`1_000_000`); prefixes for non-decimal bases (`0x`, `0b`, `0o`); no type suffixes — types are determined by inference or explicit annotation.

*Reserved keywords*: a small deliberate set. Declaration: `fn`, `type`, `agent`, `service`, `actor`, `event`, `capability`, `context`. Block-introducing: `let`, `match`, `if`, `else`, `attempt`, `recover`, `test`. Modifier and binding: `given`, `by`, `on`, `from`, `via`, `consumes`, `exports`, `invariant`, `store`, `idempotent`, `expires`. Predicate and pattern: `implies` (logical implication, `P implies Q` ≡ `!P || Q` but directional and prose-readable), `is` (pattern matching as a Boolean expression, with optional bindings: `expr is Pattern`). Type-system primitives are not keywords but built-in identifiers.

*Identifier conventions*: lowercase for context names, agent and service instances, function names, and values; uppercase for type names, constructor names, and actor types. The compiler uses case to disambiguate references in some places (module paths, constructor versus binding in patterns) — a tractable rule that experienced developers from any C-family or ML-family background already know.

### Parser tractability

The grammar is LL(k) for small k. The few non-obvious points:

Block boundaries are explicit (`{` and `}`), never inferred from indentation. Generic parameters use `[]` and never conflict with comparison or indexing; the latter is method-style (`list.at(0)`). The await marker `<-` appears only on the right-hand side of `let` bindings (v0.79: a fire-and-forget call is spelled with the distinct leading `~>` send marker, *not* `<-` — see ADR 0106; reusing `<-` for both await-and-bind and fire-and-forget would hide "does the caller wait?" at the call site). Lambda `=>` and return `->` cannot be confused because contexts differ (function header versus expression body). Module paths use `.` and resolve from the leading identifier's case (lowercase context name versus uppercase type or value name). Statement boundaries are newlines, with the parser newline-sensitive in blocks but not in parenthesised expressions or argument lists.

The trickiest piece is `let x <- expr` versus `let x = expr` — both bind a name, but one awaits an effectful call and the other binds a synchronous value. The parser distinguishes from the operator alone; the compiler enforces that `<-` appears only where an awaitable expression is on the right.

### Worked sampler

A fragment of the order-placement flow with all syntactic choices exercised:

```
on place(u: UserId, c: Cart) -> Result[Receipt, OrderError]
    given Inventories: Ref[commerce.inventory.Inventory],
          Payments:    PaymentGateway,
          Fulfilments: Ref[commerce.fulfilment.Fulfilment],
          Sagas {

  validateCart(c)?
  user := Some(u)
  cart := Some(c)

  let reservations <- reserveAll(c.items)?
  <- Sagas.compensate(() => 
       reservations.parTraverse((sku, rid) => Inventories(sku).release(rid)))?

  let authId <- Payments.authorise(c.total, u).mapErr(PaymentDeclined)?
  <- Sagas.compensate(() => Payments.refund(authId))?

  let shipId = ShipmentId.forOrder(id)
  let items  = c.items.map(i => LineItem { sku: i.sku, qty: i.qty })
  <- Fulfilments(shipId).schedule(items, u).mapErr(FulfilmentUnavailable)?
  <- Sagas.compensate(() => Fulfilments(shipId).cancel())?

  status := Placed
  Ok(Receipt { orderId: id, total: c.total })
}
```

With a ligature-aware font, this renders with → for return types, ← for awaits, ⇒ for lambdas, ≔ for storage assignment, and ∧, ∨, ¬, ≡, ≠ wherever logical and comparison operators appear. The visual density carries semantic weight without becoming opaque, and the syntactic structure remains regular enough to scan rapidly.

### Uniformity over alternative ways

A guiding principle for the surface syntax: **one canonical way to do each thing**. The language deliberately rejects features that would create syntactic alternatives to existing capabilities, on the pedagogical grounds that learners and reviewers benefit from a single idiom they recognise immediately. Where Python has accumulated multiple idioms for many tasks (list comprehensions versus map/filter, multiple string-formatting forms, `if/else` expressions versus conditional expressions, and so on), this language sides with Go's posture: fewer forms, more recognisable, less surface area to learn or argue about.

The decisions that follow:

*Pipe operator (`|>`) is not in the language.* Method chaining is the standard composition style. A pipe operator would offer a second way to express the same idea — `xs.filter(p).map(f).collect` versus `xs |> filter(p) |> map(f) |> collect` — without changing what's expressible. The language commits to method chaining. The decision can be revisited as a future enhancement if practice surfaces a strong case the principle would block, but the default is no.

*Custom operators are not in the language.* The operator vocabulary is part of the core grammar, not user-extensible. User-defined infix operators would let library authors invent their own punctuation, which is occasionally elegant in mathematical libraries but more often creates dialects that readers can't predict. Standard methods carry names; names compose better with documentation, IDE tooling, and search. The decision can be revisited if a domain emerges where the cost is high enough to justify the cost of operator dialects, but the default is no.

(Comments and documentation are committed in Section 17.)

## 17. Documentation

Most languages treat documentation as an external concern — a separate tooling pass that consumes specially-formatted comments and produces something a human reads elsewhere. The compiler doesn't know what the documentation says; the test runner ignores its examples; cross-references aren't checked. The result is documentation that drifts: references go stale, examples stop compiling, parameter descriptions outlive parameter renames.

The language commits to a different position. Documentation is part of the contract surface, attached to the declarations the architecture already names, checked by the compiler, executed by the test framework. Documentation lives where code lives, refactors with code, and stays honest because the same machinery that validates types validates the documentation that describes them.

### The form

A documentation block is delimited by `---` (alone on a line at column zero) at both ends, with a markdown body between. The convention follows existing markdown front-matter practice (Quarto, Jekyll, Pandoc), so the delimiters render as horizontal rules in any markdown viewer and the prose inside reads as prose.

```
---
Brief one-line summary used in tooltips and indexes.

Extended description. Multiple paragraphs allowed. Cross-references
to other declarations like [OrderId.fresh] or [commerce.inventory]
are validated by the compiler.

## Returns

- `Confirmed(receipt)` on success
- `OutOfStock(skus)` if reservation failed

## Example

\`\`\`ex
let result <- Orders(OrderId.fresh()).place(testUser, validCart)
expect result is Confirmed(_)
\`\`\`
---
```

The body uses paragraphs for prose, `## Section` headings for structured sections, and standard markdown for lists, code, and formatting. Internal `---` (if the prose needs a thematic break) use `***` or `___` to avoid colliding with the delimiter role.

### Placement

A doc block immediately follows the declaration it documents. For declarations with bodies (`context`, `agent`, `service`, `actor`, `capability`, handler `on`, `fn`), the doc is the first thing inside the body — after the opening `{`. For inline declarations (`type X = ...`, `invariant name: expr`, `store name: T = default`, operation signatures in a capability), the doc immediately follows the declaration on the next lines, with no intervening blank line.

```
agent Order(id: OrderId) {
  ---
  Represents a single customer order...
  ---

  store status: Cell[OrderStatus] = Pending

  invariant placed_has_user_and_cart:
    status == Placed implies (user.isSome() && cart.isSome())
  ---
  Holds for all reachable states of the agent...
  ---

  on place(u: UserId, c: Cart) -> Result[Receipt, OrderError] given ... {
    ---
    Place this order...
    ---

    validateCart(c)?
    ...
  }
}
```

The rule is consistent: doc follows the declaration. The blank line is the disambiguator — a doc block separated from anything above by a blank line is free-floating and attaches to nothing (the compiler flags this).

The placement has a folding consequence worth noting. Code folding by signature collapses both the doc and the implementation, showing just the contract. Folding by body collapses the implementation but keeps the doc visible. Two natural views, without additional tooling.

### H1 inference

The H1 of each doc block is inferred from the declaration it attaches to. The compiler emits "Context: commerce.order", "Agent: Order", "Handler: place", "Type: OrderId" — the exact formatting is a rendering choice, configurable by tool. The first paragraph of the doc body is the brief summary used in tooltips and indexes; subsequent paragraphs are extended description; `## Section` headings organise structured content.

An author may override the inferred H1 by writing `# Some Heading` as the first line of the doc body. This is useful when the declaration name doesn't read as good prose — particularly for invariants, where `placed_has_user_and_cart` is the identifier and "Placed orders have user and cart recorded" is the readable form.

### Recognised sections

Tooling recognises a starter set of `## Section` headings and parses them structurally:

- `## Parameters` — description of parameters
- `## Returns` — description of return value and its variants
- `## Errors` — description of error variants
- `## Example` / `## Examples` — executable examples
- `## Invariants` — properties preserved (cross-reference to formal invariants)
- `## Dependencies` — contexts this declaration depends on
- `## Lifecycle` — state-machine description for stateful declarations

Other markdown content renders as normal prose. The recognised set is open to extension as patterns demand.

### Cross-reference validation

References take the form `[Name]`, `[Context.Name]`, or `[Type.method]` — wiki-style links resolved against the declaration graph. The compiler validates them at build time: `[OrderId.fresh]` must refer to a real function on `OrderId`; `[commerce.inventory]` must refer to a real context; `[place]` (bare) refers to a same-scope declaration. Renaming a referent updates references through the language server; broken references are warnings, configurable to errors.

This is the single largest win over conventional documentation. The drift problem either disappears or surfaces immediately — references stay honest because they are checked the same way type references are.

### Executable examples

Code blocks marked with the `ex` language hint are run by the test framework as doctests. Test capabilities apply by default; the doctest sees a `TestClock`, a `TestPayment`, and the rest of the standard test environment. Examples that fail to compile or fail their assertions fail the build. Documentation that says "use it like this" actually has to be usable like that.

Other code-block language hints have other meanings: `code` (or unmarked) for non-executed examples; `repl` for input/output transcripts. The `ex` form is the load-bearing one.

### Enforcement

Publicly-exported declarations (types, functions, agents, services, events, capabilities) require documentation; the compiler warns if it is missing. Internal helpers do not require it. Projects may configure the strictness — a stricter project may escalate the warning to an error, or relax it entirely.

This is a soft pedagogical nudge rather than absolute force. The architecture says public means contract, and contracts deserve description; the compiler reminds without compelling.

### Tutorial-level documentation

For longer-form prose that does not attach to a single declaration — design rationale, concept introduction, walk-throughs — `doc/` files alongside source carry markdown with the same cross-reference validation. A `doc/sagas.md` file explaining how the `Sagas` capability expresses compensation across bounded contexts can reference `[commerce.order.Order.place]` and the build validates it. These files participate in the same generated documentation site as in-source docs.

### The full comment story

With documentation as first-class, the three-level comment vocabulary reads cleanly:

- `--` — inline explanatory comment. A note to the next reader, debugging context, "this looks odd because…". Renders as a long dash with ligatures. One per line.
- `--- ... ---` — documentation block. Markdown body. Cross-references validated. Examples executed. Attached to the immediately-following declaration.
- `{- -}` — block comment for code commentary, not documentation. Useful for temporary commenting-out; rare in well-written code.

Two dashes for a note, three-dashes-delimited block for the contract, braces for bracketed prose. Each form does one job and the visual weight matches the role.

### Output targets

The same source feeds multiple rendering targets without modification: IDE tooltips show the H1 and summary paragraph; API reference pages render the full body with sections and cross-references hyperlinked; search indexes use H1, summary, and section headers; doctests run as part of the test suite; a generated documentation site organises everything by context with the architectural graph visible. Type refinements (Section 15) participate directly in this rendering: a refined type's constraint appears in tooltips, type reference pages, and search-indexable metadata as part of the type's contract — a reader sees `VoucherCode` defined as `String where Matches("[A-Z0-9]{8}")` without having to consult the source. The same refinements drive generated external schema artifacts (OpenAPI for HTTP services, AsyncAPI for events, JSON Schema for storage migrations) produced at build time. The doc tooling is decoupled from the language proper — the language commits to the form and the AST associations; rendering is a downstream concern with multiple consumers.

## 18. Runtime and Platform Relationship

The language describes the application; the compiler and runtime map it to a platform.

For Cloudflare Workers as the first target:

- Agents → Durable Object classes
- Services → Worker entrypoints
- Storage types → SQLite-in-DO, KV, or in-memory, chosen by access pattern and refinement annotations
- `Ref[A]` → Durable Object stub
- Capabilities → bindings (the wrangler configuration is generated from the program)
- Messages → RPC invocations or `fetch`
- Handler atomicity → DO input gate semantics
- Per-sender FIFO → runtime-provided message ordering

A note on portability. Bynk's architecture is *informed by* Cloudflare's specific primitives, not just compiled to them. Handler atomicity comes from the DO input/output gate; per-sender FIFO comes from the DO's serial input; held connections survive on hibernatable WebSockets; the "one Worker per bounded context" deployment shape mirrors Cloudflare's deployment unit. The platform abstraction is designed so other actor runtimes (BEAM, Akka, Convex) could host Bynk in principle — but those runtimes provide these guarantees in materially different ways, and the architecture would not have these specific shapes if built for them. Portability is plausible rather than load-bearing. A Bynk-on-BEAM port is a reasonable future project; it would not be free, and some current shapes (notably the way held connections live in agent state and survive hibernation) would need rework on platforms without an exact analogue. Cloudflare is the load-bearing target.

### First-party capabilities as the extension surface

The language core stays minimal: bounded contexts, services, agents, value types, handlers, storage primitives, capabilities as a mechanism, the query algebra, the failure model, the type system core, the documentation system. What's deliberately *not* in the core: durable workflow orchestration, retry policies with backoff, structured rate limiting, schema migration tooling, distributed tracing, batch processing patterns. Each of these is useful; each is canonically solved by a *capability* rather than by a language feature.

**First-party capabilities** are those the Bynk project itself supplies as the canonical answer to a common architectural concern. They live in framework contexts that application contexts consume; their implementations are ordinary Bynk code; their surfaces are ordinary `given` clauses; there is no special compiler support and no syntactic distinction between using a first-party capability and using one written by an application team or a third party. The first committed first-party capability is `Sagas` — durable workflow compensation through forward/undo step pairs, demonstrated in Section 20 — with others emerging as the language matures.

The pedagogical layering of Section 2 maps onto this stratification. The foundations layer uses no capabilities at all; the coordination layer uses platform capabilities (`Clock`, `Http`, `Events`); the advanced layer uses first-party and domain-specific capabilities (`Sagas`, others). A student meets only the capabilities they need for what they're building.

The benefit of this commitment is that the language stays learnable while the cost surface for harder problems lives in capabilities where they can evolve, version, and improve without language churn. Adding a new capability is additive: it doesn't change the meaning of existing programs and doesn't require revisiting the language specification. The extension story is uniform — third parties build the same way the project does — and the line between "what the language solves" and "what a framework solves" is visible in the source. The cost is that some problems with elegant language-level solutions (durable on-abort being the canonical example) are solved through a slightly more verbose capability surface instead. The trade is deliberate: a tight language with rich capabilities, rather than a sprawling language that hides its costs.

## 19. Compilation Strategy

The architecture's specific commitments — bounded contexts, agents-as-Durable-Objects, services-stateless, capability injection, atomic handlers, cross-cutting concerns as capabilities, the per-sender FIFO floor — constrain the compilation problem more tightly than a general-purpose language would. Five facts shape the strategy:

1. The primary target is Cloudflare Workers + Durable Objects: a V8 isolate runtime with a JavaScript host, the DO input/output gate model providing per-agent atomic handlers, and a small set of platform primitives (Queues, Service Bindings, scheduled triggers, hibernatable WebSockets).
2. The language has a platform abstraction (Section 18) designed so other targets with similar primitives could in principle be supported, though the architecture is shaped by Cloudflare's specific affordances; platform-specific glue is injected at link time.
3. Each agent must compile to something the platform implements per-sender-FIFO and atomic-commit for. On Cloudflare that is a DO class with its input gate; on other platforms the mechanism differs but the contract is the same.
4. The type system is fully static and type-erasable; types do not appear at runtime.
5. The audience needs debuggable output, fast iteration, readable error messages, and the ability to inspect generated code when something goes wrong.

### Three-tier output

**Tier 1 — generated TypeScript.** The compiler emits readable, typed TypeScript that preserves source-level identifiers, carries source maps for stack traces and debuggers, and lowers high-level constructs (pattern matching, sum-type discrimination, `?` propagation, `is` and `implies`, comprehensions and combinator chains) to ordinary TypeScript idioms. Each agent compiles to a `DurableObject`-shaped class; each service compiles to a Worker handler or routes table; each context becomes a TypeScript module, and in the default deployment shape its own Worker bundle. Cross-cutting concerns — saga compensation, idempotency, tracing, retries — are not in the language and have no special lowering; they flow through as ordinary capability calls invoking provider implementations from the runtime library. **(v0.68–v0.71: the source-map commitment is realised — `bynk-emit` emits a sibling `.ts.map` + `//# sourceMappingURL` per file, line-level and statement-anchored, per-statement through handler/test bodies (v0.70); ADR 0103. Phase-8 bundle composition is confirmed: `wrangler`/esbuild folds the per-file maps into the worker bundle, so a `.bynk` breakpoint resolves on the running worker under `bynk dev --inspect`, v0.71.)**

**Tier 2 — the runtime library.** A small hand-written TypeScript library that the generated code imports. It provides the constructs that would be tedious or error-prone to inline at every call site: sum-type discriminant helpers, pattern-match support, the `Result`/`Option` machinery with `?` desugared against it, the event-bus dispatcher, the capability resolver, and the default providers for the standard capabilities (in-memory `Sagas`, in-memory `Idempotency`, no-op `Tracer`, console `Logger`). Hand-written rather than generated so it can be reviewed, optimised, tested, and debugged independently of the compiler.

**Tier 3 — platform bindings.** Per-target glue that maps capability types and storage shapes onto the target platform's primitives. The Cloudflare binding maps `Map[K, V]` storage to DO storage with batched gets and puts at the gate boundary; the `Events` capability to a runtime event bus implemented over Queues; `Clock`, `Crypto`, `Fetch` to V8 and Workers built-ins; `Connection[F]` to hibernatable WebSocket sessions; `Ref[A]` to DO stubs and Service Bindings. Other platforms have their own bindings exposing the same capability surface. User source never imports platform-specific identifiers; the compiler injects the binding at link time based on build target.

### Why TypeScript output, not WASM or plain JS

WASM gives compute performance the language does not need. The bottleneck in this style of code is cross-agent I/O and storage access, not arithmetic; compute-heavy work can be encapsulated in platform-supplied capabilities where appropriate. WASM also makes source-level debugging considerably harder, which is wrong for the educational target. As an optional future backend for specific compute-heavy capability implementations WASM is reasonable; as the primary output it is not.

Plain JavaScript would work but loses meaningful benefits. Typed output documents the lowering for readers, surfaces compiler bugs as TypeScript type errors at the boundary with the runtime library, integrates with Cloudflare's TypeScript-first toolchain defaults, and gives developers something readable when they need to inspect compiler output. The runtime cost is identical — TypeScript compiles to JavaScript, which is what Workers run.

### Compilation pipeline

Eight phases, in source-to-target order:

1. **Parsing** — AST with full position information for source maps and error messages. The grammar is LL(k); standard recursive-descent.
2. **Name resolution** — resolve identifiers to declarations, validate `[Name]` and `[Context.Name]` documentation cross-references, build the inter-context dependency graph.
3. **Type inference** — Hindley-Milner with extensions for closed sums, opaque types (encoded as branded types in TypeScript output), capability sets, generic storage shapes. Annotations required at contract boundaries (Section 15); inferred internally.
4. **Effect and capability analysis** — verify that every effect used in a body is declared in the enclosing `given` clause; verify pure functions stay pure; track capability provenance.
5. **Architectural validation** — services stateless (no `store` fields in services), no shared mutables, storage only inside agents, invariant predicate well-formedness, pattern coverage on event subscriptions, exhaustiveness on `match`, capabilities referenced are declared in `given`.
6. **Lowering to IR** — a smaller intermediate language that simplifies code generation. Desugar `?` to explicit Err propagation; pattern-match to discriminant-and-extract; `is` and `implies` to Boolean expressions. No special lowering for compensation or idempotency — those are ordinary capability calls.
7. **Code generation** — emit TypeScript per IR module. Agents implement a `DurableObject`-shaped interface; services export Worker handlers; event subscribers register with the runtime event bus at module load.
8. **Bundling and source maps** — produce one or more Worker scripts with source maps, ready for `wrangler deploy` (or the equivalent on other platforms).

### Lowerings for architectural primitives

The interesting compilation choices map directly onto the architecture:

- **Agent declaration** → DO class with storage fields backed by DO storage. Reads on first access cache for the duration of the handler; writes batch at handler-commit time via the input/output gate. Invariants are predicates evaluated at the commit boundary; failure aborts the transaction.
- **Service declaration** → Worker handler function that performs protocol parsing, actor authentication, message construction, agent dispatch, and response shaping. Stateless: no DO, no persistent storage.
- **Event emission** → call to the runtime event bus capability injected at the binding. On Cloudflare this maps to Queues with topic-as-queue routing, or a custom event-fanout DO for higher-fanout scenarios.
- **Cross-agent call (`Ref[A]`)** → DO stub lookup then RPC method call. Per-sender FIFO comes free from the DO's input gate.
- **Capability use** → method call on an injected runtime object. The platform binding decides what that object is (a `Clock` becomes a `Date.now()` wrapper; a `PaymentGateway` becomes a fetch-based adapter to an external processor; the in-memory `Sagas` provider becomes a handler-local registration list with an abort hook attached to the handler's try/catch; the in-memory `Idempotency` provider becomes a per-handler dedup `Map` lookup with cache write on outcome). Cross-cutting concerns flow through this same mechanism — no special lowering required.
- **Test declarations** → compiled to entries in a test-runner registry that wires test capabilities, executes the body, and reports outcomes. Tests run at build time in CI and on demand locally.

### Bundling and deployment shape

The default mapping is **one Worker bundle per bounded context, with its own `wrangler.toml` (or `wrangler.jsonc`) deployment configuration generated by the compiler**. The configuration encapsulates everything the context needs to run on Cloudflare: the Worker entry point (the bundled TypeScript module), the Durable Object class bindings (one per agent in the context), Service Bindings to other contexts the current context `consumes`, Queue and KV bindings for the platform-supplied capabilities used, and any other platform resources the source has declared a dependency on. The developer never hand-writes Wrangler config; the source *is* the deployment specification, and the compiler emits the corresponding configuration alongside the Worker bundle.

Cross-context calls compile to invocations through Service Bindings: a `Ref[commerce.inventory.Inventory]` in the order context's source resolves at compile time to "look up the `Inventory` DO stub through the `INVENTORY` Service Binding, then invoke its handler method." Each context gets its own deployment lifecycle, its own runtime environment, its own observability surface, and the platform's typed cross-Worker RPC for inter-context calls — all without the developer thinking about Wrangler configuration.

Monolithic deployment — all contexts in a single Worker — remains available as a build-time configuration for small applications or for early-stage development where per-context Workers' deployment overhead isn't yet worth it. The choice is never a source-level concern; the source describes the architecture, and the build configuration describes how it's mapped onto deployment units.

### Local development

The development experience aims for the immediacy of a Vite-style loop: save a file, see the new behaviour in seconds, with logs streamed and errors surfaced inline. The architecture supports this naturally because Cloudflare's runtime is open-source: **`workerd`** is the same engine that runs Workers in production, available as a self-contained binary that the language's tooling embeds for local development.

The dev loop:

1. The developer saves a source file. The watcher detects the change.
2. The compiler recompiles affected contexts incrementally — front-end phases (parsing, name resolution, type inference, validation) cache results, and only contexts whose source has changed or whose dependencies have changed get regenerated.
3. `workerd` picks up the regenerated TypeScript output and reloads the affected Workers. Reload is process-level rather than hot-module-replacement: DO state in the dev session is reset on reload, which is the right default for development (clean slate between iterations) and can be opted out of where state needs to persist via a snapshot/restore mechanism the dev server exposes.
4. The developer sees the new behaviour, with logs streamed to the terminal and source maps mapping stack traces back to the original source files.

The dev server is a single command (e.g. `lang dev`) that orchestrates all contexts in the workspace, configures `workerd` with the inter-context Service Bindings wired locally, runs the file watcher and incremental recompiler, serves the HTTP endpoints, and streams logs. The same setup runs the test runner in watch mode when requested — saving a source file causes affected tests to re-run automatically.

This matches the experience of frontend development with Vite or backend development with `air` (Go) or `cargo watch` (Rust), adapted to the architecture's specifics. The point of friction `workerd` removes — running a full Cloudflare Workers environment locally without deploying or emulating — is the load-bearing one. Production parity is structural rather than aspirational: the same engine runs the same compiled output in development and production.

### Project structure and file organisation

A convention rather than a hard language requirement, but the compiler's default resolution and the build tooling assume it. Dotted context names map to directory paths; the last segment names the file:

```
commons commerce.money         →  src/commerce/money.bynk
context commerce.inventory     →  src/commerce/inventory.bynk
context hotel.rooms            →  src/hotel/rooms.bynk
context hotel.bookings         →  src/hotel/bookings.bynk
```

Source files use the `.bynk` extension. The root (`src/` by default) is configurable in the build configuration for projects with different layouts.

For contexts large enough to warrant splitting across multiple files, directory expansion applies: if `src/hotel/bookings/` exists as a directory, all `.bynk` files in it belong to the `hotel.bookings` context. The single file `src/hotel/bookings.bynk` is the alternative when one file suffices. Files within a directory can be named freely; their declarations share the context's namespace.

```
src/
├── hotel/
│   ├── rooms.bynk               -- single-file context
│   └── bookings/                -- multi-file context
│       ├── agents.bynk
│       ├── types.bynk
│       └── services.bynk
```

Tests live wherever suits the project. A `suite` names its target and is legal in any file, so tests may sit beside the source they exercise or under a conventional `tests/` tree — the placement carries no meaning, because test-ness comes from the `suite` declaration, not a filename:

```
src/commerce/
├── orders.bynk                 -- context commerce.orders
├── orders.suite.bynk           -- suite commerce.orders (a conventional name, not required)
├── money.bynk                  -- commons commerce.money
└── inventory.bynk              -- context commerce.inventory
tests/
└── commerce/
    └── orders.bynk             -- further suites, gathered under a tests/ root
```

Discovery walks the project's configured `include` trees for `suite` declarations rather than keying off a `.test.bynk` suffix; an `exclude` list prunes what it should skip. Both a conventional layout (a `src` tree plus `tests`) and a flat single-root project work without configuration. A `.bynk` file may even hold a `context` and its `suite` together — the shareable, single-file shape the in-browser playground uses. The build strips the suites; the test runner collects and executes them.

First-party capabilities supplied by the Bynk project itself follow the same naming convention but live under a separate root (`lib/` or `framework/` depending on packaging), making application code visually distinct from project-supplied capabilities. The capability context `bynk.sagas` (Section 18) maps to `lib/bynk/sagas.bynk` or `lib/bynk/sagas/` if multi-file.

The convention is enforced as a default by the compiler — context references resolve against the expected paths — but overridable through build configuration. The point is to make the architectural graph visible in the filesystem: a reader navigating `src/` can see the bounded contexts as directories and files without needing to load the source.

### Bootstrap language

The compiler is written in **Rust** (workspace crates `bynkc`, `bynk-fmt`,
`bynk-lsp`, `bynk-grammar`; Rust 2024 edition, MSRV 1.95). Rust gives the best
ergonomics for compiler-shaped code — algebraic data types, exhaustive pattern
matching, and recursive descent with rich error recovery — plus the right
ecosystem and single-binary distribution. The costs are a steeper contributor
on-ramp and longer build times, judged worth paying for a substantial compiler.

> **Decision history.** An earlier draft of these notes proposed **Go** as the
> bootstrap language, trading compiler ergonomics for distribution velocity and
> familiarity, with Rust listed as the strongest alternative and the switch
> called "genuinely revisitable". The project made that switch up front: the
> implementation has been Rust from the start. The Go rationale is retained here
> only as the record of a considered-and-rejected option.

Alternatives considered:

- *Go* — single-binary distribution and a fast build, familiar to many engineers
  on Cloudflare, but genuinely awkward for compiler work: the
  sealed-interface-plus-type-switch workaround for ADTs adds verbosity and
  review-time noise at every pattern-match.
- *TypeScript* — same ecosystem as the output and runtime library; lowest
  impedance for contributors familiar with the Cloudflare stack. TypeScript at
  scale is even harder than Go for compiler work, and the lack of single-binary
  distribution adds friction.

Self-hosting (compiling Bynk with Bynk) remains a long-term aspiration but isn't
a v1 goal.

### Tooling around the compiler

The compiler alone is not the deliverable. The v1 experience requires:

- **Dev server** — file watcher, incremental compiler, `workerd` orchestrator with inter-context Service Bindings wired locally, test runner in watch mode, log streamer. The single-command development experience.
- **LSP server** — type-aware completion, go-to-definition, hover documentation, refactoring. Built on the compiler's name-resolution and type-inference phases.
- **Formatter** — opinionated, no configuration. Reads AST, prints canonical source.
- **Documentation generator** — already specified in Section 17; reuses the compiler's name resolution to validate `[Name]` cross-references and emit a generated docs site.
- **Schema generator** — derives external schema artifacts from refined type definitions: OpenAPI for HTTP service contracts, AsyncAPI for event topics, JSON Schema for storage layouts and migration boundaries. The same refinements that drive compile-time type identity and runtime validation drive the wire-level schemas — one source, multiple downstream consumers, no hand-maintained correspondences.
- **Test runner** — runs `test` declarations with test capabilities by default, reports outcomes structurally.

A REPL — once flagged here as "ambitious and probably v2 or v3" — **has shipped** as the in-browser playground (the in-browser track, ADRs 0136–0140): the compiler runs in the browser as wasm, compiling and executing the in-process subset with no install. A debugger plugin for VS Code is desirable but follows the LSP work.

### What is deferred

- **Whole-program optimisation** beyond what the V8 JIT and TS tree-shaking provide: not required at the scale the language initially targets.
- **Incremental compilation** across source changes: full recompilation is fast enough at v1 scale; incremental builds become worthwhile when codebases grow.
- **Additional backends** (native, mobile): out of scope for the foundation; the platform-abstraction layer leaves the door open in principle without committing to specific targets, though porting away from Cloudflare would require rework of the shapes that lean most heavily on its primitives (Section 18). A **Browser** platform binding has since landed (the in-browser track, ADR 0138) — but scoped to the playground and education, not a production browser-app target; native and mobile remain deferred.
- **Self-hosting**: long-term, not v1.

## 20. Worked Examples

Two examples exercise the architecture in complementary ways. The first walks through synchronous request-response with multi-context coordination — bounded contexts, cross-context refs, the service/agent split, actor-as-contract, outcome handling, agent invariants, the `Sagas` capability for compensation, and the `Idempotency` capability for handler-level dedup. The second walks through real-time interaction with persistent typed connections — WebSocket protocol, held connections as agent state, broadcast, event emission for cross-context fan-out, and idempotency keyed on client-supplied identifiers.

### Example 1: Order placement

A small order-placement flow exercising bounded contexts, cross-context refs, the service/agent split, actor-as-contract, outcome handling, agent invariants, and the `Sagas` capability for cross-context compensation.

```
commons commerce.money {
  type CurrencyCode = String where Matches("[A-Z]{3}")
  type Money        = { minorUnits: Int, currency: CurrencyCode }
  exports transparent { Money, CurrencyCode }
}

context commerce.inventory {
  uses commerce.money

  type Sku            = String where Matches("[A-Z0-9]{3,16}")
  type Quantity       = Int where InRange(1, 9999)
  type ReservationId  -- opaque, identity
  type Reservation    = { 
    id: ReservationId, sku: Sku, qty: Quantity, 
    orderId: OrderId, expiresAt: Instant 
  }
  type ReserveOutcome =
    | Reserved(ReservationId)
    | InsufficientStock(available: Int, requested: Quantity)

  exports transparent { Sku, Quantity, ReserveOutcome }
  exports opaque      { ReservationId }

  agent Inventory(sku: Sku) {
    store available:    Cell[Int where NonNegative]     = 0
    store reservations: Map[ReservationId, Reservation] = {}

    invariant available_non_negative:
      available >= 0

    on reserve(qty: Quantity, orderId: OrderId) -> ReserveOutcome
        given Clock, Idempotency {
      <- Idempotency.dedup(on: orderId, expiresAfter: 24h)?
      if available < qty {
        InsufficientStock(available: available, requested: qty)
      } else {
        let now = <- Clock.now()
        let rid = ReservationId.fresh()
        <- available.update(a => a - qty)
        <- reservations.put(rid, Reservation { 
          id: rid, sku, qty, orderId, expiresAt: now + 15.minutes 
        })
        Reserved(rid)
      }
    }

    on release(rid: ReservationId) by InternalCaller
        given Idempotency {
      <- Idempotency.dedup(on: rid, expiresAfter: 1h)?
      let existing <- reservations.get(rid)
      match existing {
        Some(r) => {
          <- available.update(a => a + r.qty)
          <- reservations.remove(rid)
        }
        None => ()
      }
    }
  }
}

context commerce.payment {
  uses commerce.money

  type AuthId         -- opaque, identity
  type PaymentError = Declined(reason: Text) | InsufficientFunds | GatewayDown
  exports opaque      { AuthId }
  exports transparent { PaymentError }

  -- PaymentGateway is platform-supplied: a capability provided by the binding 
  -- to an external processor (Stripe, Adyen, etc).
  capability PaymentGateway {
    authorise(amount: Money, userId: UserId) -> Result[AuthId, PaymentError]
    refund(authId: AuthId)   -- fire-and-forget, idempotent
  }
}

context commerce.fulfilment {
  uses commerce.money
  consumes commerce.inventory   -- for Sku

  type ShipmentId     -- opaque, identity, with .forOrder(OrderId) constructor
  type LineItem       = { sku: Sku, qty: Quantity }
  type ScheduleError  = NoCourier | ServiceDown

  exports opaque      { ShipmentId }
  exports transparent { LineItem, ScheduleError }

  agent Fulfilment(id: ShipmentId) {
    store scheduled: Cell[Bool]            = false
    store items:     Cell[List[LineItem]]  = []
    store recipient: Cell[Option[UserId]]  = None

    on schedule(its: List[LineItem], u: UserId) -> Result[Unit, ScheduleError] {
      if scheduled {
        Ok(())            -- idempotent re-schedule
      } else {
        items     := its
        recipient := Some(u)
        scheduled := true
        Ok(())
      }
    }

    -- idempotent: cancel on an already-cancelled shipment is a no-op
    on cancel by InternalCaller {
      scheduled := false
      items     := []
      recipient := None
    }
  }
}

context commerce.order {
  uses commerce.money
  consumes commerce.inventory
  consumes commerce.payment
  consumes commerce.fulfilment

  type OrderId     -- opaque, identity
  type CartItem    = { sku: Sku, qty: Quantity, unitPrice: Money }
  type Cart        = { items: List[CartItem], total: Money }
  type OrderStatus = Pending | Placed | Cancelled
  type Receipt     = { orderId: OrderId, total: Money }

  type OrderError =
    | OutOfStock(items: List[Sku])
    | PaymentDeclined(reason: PaymentError)
    | FulfilmentUnavailable(reason: ScheduleError)
    | InvalidCart(reason: Text)

  exports opaque      { OrderId }
  exports transparent { Cart, CartItem, Receipt, OrderError }

  actor User {
    auth     = BearerToken verified by JwtVerifier
    identity : UserId from claim "sub"
  }

  service CustomerApi from HTTP {
    on POST "/orders" by user: User (cart: Cart) given Orders: Ref[Order] {
      match <- Orders(OrderId.fresh()).place(user.identity, cart) {
        Ok(receipt)                        => Response.created(receipt)
        Err(OutOfStock(items))             => Response.conflict({ outOfStock: items })
        Err(PaymentDeclined(reason))       => Response.paymentRequired({ reason })
        Err(FulfilmentUnavailable(reason)) => Response.serviceUnavailable({ reason })
        Err(InvalidCart(reason))           => Response.badRequest({ reason })
      }
    }
  }

  agent Order(id: OrderId) {
    store status: Cell[OrderStatus]    = Pending
    store user:   Cell[Option[UserId]] = None
    store cart:   Cell[Option[Cart]]   = None

    invariant placed_has_user_and_cart:
      status == Placed implies (user.isSome() && cart.isSome())

    on place(u: UserId, c: Cart) -> Result[Receipt, OrderError]
        given Inventories: Ref[commerce.inventory.Inventory],
              Payments:    PaymentGateway,
              Fulfilments: Ref[commerce.fulfilment.Fulfilment],
              Sagas {

      validateCart(c)?
      user := Some(u)
      cart := Some(c)

      let reservations <- reserveAll(c.items)?
      <- Sagas.compensate(() => 
           reservations.parTraverse((sku, rid) => Inventories(sku).release(rid)))?

      let authId <- Payments.authorise(c.total, u).mapErr(PaymentDeclined)?
      <- Sagas.compensate(() => Payments.refund(authId))?

      let shipId    = ShipmentId.forOrder(id)
      let lineItems = c.items.map(i => LineItem { sku: i.sku, qty: i.qty })
      <- Fulfilments(shipId).schedule(lineItems, u).mapErr(FulfilmentUnavailable)?
      <- Sagas.compensate(() => Fulfilments(shipId).cancel())?

      status := Placed
      Ok(Receipt { orderId: id, total: c.total })
    }

    fn validateCart(c: Cart) -> Result[Unit, OrderError] {
      if c.items.isEmpty() { Err(InvalidCart("empty cart")) } else { Ok(()) }
    }

    fn reserveAll(items: List[CartItem]) -> Result[Map[Sku, ReservationId], OrderError]
        given Inventories: Ref[commerce.inventory.Inventory] {
      let outcomes <- items.parTraverse(item =>
        Inventories(item.sku).reserve(item.qty, id).map(o => (item.sku, o))
      )

      let reserved  = outcomes.collect { (sku, Reserved(rid))         => (sku, rid) }.toMap()
      let shortfall = outcomes.collect { (sku, InsufficientStock _)   => sku }

      if shortfall.nonEmpty() {
        <- reserved.parTraverse((sku, rid) => Inventories(sku).release(rid))
        Err(OutOfStock(shortfall))
      } else {
        Ok(reserved)
      }
    }
  }
}
```

A reader of this file can identify the architectural facts by glyph alone. One commons and four bounded contexts cooperate: `commerce.money` is a commons holding shared value vocabulary (Money, CurrencyCode); `commerce.inventory` owns stock state and reservations; `commerce.payment` is the boundary to an external processor; `commerce.fulfilment` owns shipment scheduling; `commerce.order` orchestrates placement. The four contexts each declare two distinct kinds of import: `uses commerce.money` brings the shared vocabulary into scope (no runtime coupling); `consumes commerce.inventory` (and similar) declares a behavioural dependency on another context. Each context exports a small public surface — types it owns (typically opaque for identifiers, transparent for value vocabulary) — and the dependency graph is in the code: a reviewer can see at a glance that `commerce.order` consumes three downstream contexts and uses one commons, and that those downstream contexts share vocabulary but don't depend on each other behaviourally.

The service handler in `commerce.order` does only HTTP work. The actor type handles authentication; the parameter type handles parsing; the call to the agent is one line; the typed `Result[Receipt, OrderError]` is translated into HTTP responses by one match. No domain logic; no orchestration; no compensation. A reviewer would push back if any of these crept in.

The `Order.place` handler is where the cross-context saga is expressed. After local validation and state initialisation, three steps run in sequence: reserve inventory across SKUs (delegated to the `reserveAll` helper, which handles its own partial-failure cleanup since the failures within reservation are typed outcomes returned from peer agents); authorise payment via the platform's `PaymentGateway` capability; schedule fulfilment in another bounded context. Each step is an awaited cross-agent (or cross-capability) call returning a `Result`; each successful step is followed by an awaited `Sagas.compensate` call registering a compensation. If any step returns Err — payment declined, fulfilment unavailable — the handler exits Err: the `Sagas` provider's abort hook runs through registered compensations in LIFO order, each awaited and wrapped in a best-effort `attempt`, and the Err propagates to the caller. The reservations are released; the payment auth is refunded; nothing is left in an inconsistent state. The variant carried in `OrderError` names what went wrong in domain terms, and the service handler's match on `Result` converts that into the correct HTTP response.

The compensation pattern is honest about what it cannot guarantee. Compensations run as awaited calls during the unwind, but they cannot guarantee remote success — `Inventory.release`, `Payment.refund`, `Fulfilment.cancel` could themselves fail at the moment of compensation. The targets must be idempotent (release on a missing reservation id is a no-op; cancel on an already-cancelled shipment is a no-op; refund is documented as idempotent by the gateway capability), so a retry from the runtime's at-least-once delivery produces the right result if the first attempt didn't. The fifteen-minute reservation expiry on the `Inventory` side is the safety net for the case where compensation itself fails entirely — eventually-consistent recovery via temporal decay.

The agent invariants make local guarantees visible at the boundary of each handler commit. `Order.placed_has_user_and_cart` captures that a `Placed` order necessarily has a user and a cart recorded; a future refactor that accidentally re-ordered the status assignment ahead of the cart store would fail at handler commit rather than silently producing inconsistent state. `Inventory.available_non_negative` captures the simple property that available stock cannot go negative; the reserve handler's guard makes this true by construction, and the invariant ensures any future change preserves it.

Looking across the contexts: services dispatch and format; agents orchestrate via local helpers; cross-context flows use the `Sagas` capability for structured compensation; idempotency is visible at the compensation targets via the `Idempotency` capability; invariants capture what each agent always preserves locally; opaque types prevent cross-context construction of identifiers; transparent types are the deliberately shared value vocabulary. Atomicity is local to each agent commit; consistency across contexts is established by the saga-unwinding and the targets' idempotency requirements; the architecture is what tells the reader where each concern lives.

The refined value types are doing work the example doesn't have to spell out. `Sku = String where Matches("[A-Z0-9]{3,16}")` means every `Sku` value in the system — in storage, in arguments, in event payloads, in HTTP request bodies — has been validated against that pattern by the type's constructor. `Quantity = Int where InRange(1, 9999)` means no handler ever sees a zero or negative quantity; the reserve handler's `qty: Quantity` parameter is *constructively* positive, and the reservation store's `qty: Quantity` field carries the same guarantee through to durable storage. `CurrencyCode = String where Matches("[A-Z]{3}")` validates ISO 4217 shape at every boundary. When the HTTP service handler deserialises a request body containing a `Cart`, every nested `Sku`, `Quantity`, and `CurrencyCode` is validated at the framework boundary; a malformed request produces a structured 400 response and the handler body never runs. Generated OpenAPI specs for the HTTP service expose the same constraints to API consumers. One definition per type; compile-time identity, runtime validation, external schema — all derived from the `where` clause on the type definition.

A primitive-choice decision worth flagging: `Money` is `{ minorUnits: Int, currency: CurrencyCode }` rather than `{ amount: Float, currency: CurrencyCode }`. Integer minor units (pennies, cents, fils) are exact under all arithmetic, eliminate the entire class of decimal-precision bugs, and match how production payment systems (Stripe, Square, most gateways) represent money internally. The currency field carries the per-currency subdivision needed at display time. This is the cleaner choice for service-tier financial code, and it illustrates a discipline that compounds with refinement: choosing the right primitive eliminates classes of problems before refinement needs to solve them. A fractional type with a `Scale(2)` predicate would have been a less satisfying way to chase a problem that better representation makes structural.

### Example 2: A chat-room with held connections and event fan-out

A real-time chat-room flow exercising the WebSocket protocol, held connections as typed agent state, broadcast to multiple participants, event emission for cross-context fan-out, and handler-level idempotency keyed on a client-supplied message identifier. It demonstrates patterns the order example does not reach: long-lived runtime resources (`Connection[F]`) flowing from a service to an agent at acceptance and persisting in agent state across handler invocations; at-most-once WebSocket sends co-existing with at-least-once event delivery and storage commits, all within a single handler; and the fan-out from a single domain event to subscribers in another context.

```
context chat.shared {
  type RoomId          -- opaque, identity
  type UserId          -- opaque, identity
  type MessageId       -- opaque, identity
  type ClientMessageId -- opaque, identity (client-supplied, stable across retries)

  type Message = {
    id:      MessageId,
    roomId:  RoomId,
    sender:  UserId,
    content: Text,
    sentAt:  Instant,
  }

  type ServerFrame =
    | MessageBroadcast(Message)
    | UserJoined(UserId)
    | UserLeft(UserId)
    | SystemMessage(Text)

  type ClientFrame =
    | Send(content: Text, clientMsgId: ClientMessageId)
    | Typing(active: Bool)

  exports opaque      { RoomId, UserId, MessageId, ClientMessageId }
  exports transparent { Message, ServerFrame, ClientFrame }
}

context chat.rooms {
  consumes chat.shared

  event MessageSent {
    roomId:  RoomId,
    sender:  UserId,
    content: Text,
    sentAt:  Instant,
  }

  event UserJoinedRoom { roomId: RoomId, user: UserId }
  event UserLeftRoom   { roomId: RoomId, user: UserId }

  exports events { MessageSent, UserJoinedRoom, UserLeftRoom }

  actor Participant {
    identity: UserId
    -- authorisation invariant: caller must be permitted to access the
    -- room identified in the request parameters; auth scheme supplied
    -- by the platform's Permissions capability.
  }

  service ChatGateway from websocket(in: ClientFrame, out: ServerFrame) {
    on open by user: Participant (params: { roomId: RoomId })
        given Rooms: Ref[Room] {
      <- Rooms(params.roomId).join(user.identity, connection)
    }

    on close by user: Participant (params: { roomId: RoomId })
        given Rooms: Ref[Room] {
      <- Rooms(params.roomId).leave(user.identity, connection)
    }

    on message(frame: ClientFrame) by user: Participant (params: { roomId: RoomId })
        given Rooms: Ref[Room] {
      match frame {
        Send(content, clientMsgId) =>
          <- Rooms(params.roomId).post(user.identity, content, clientMsgId)
        Typing(_) => ()    -- presence indication; ignored in this example
      }
    }
  }

  agent Room(id: RoomId) {
    store members:     Set[UserId]                                = {}
    store connections: Map[UserId, Connection[ServerFrame]]       = {}
    store history:     Log[Message]                               @retain(30.days)

    invariant connections_subset_of_members:
      connections.keys.all(u => members.contains(u))

    on join(u: UserId, conn: Connection[ServerFrame]) given Events {
      let isNewMember = <- !members.contains(u)
      <- members.add(u)
      <- connections.put(u, conn)

      if isNewMember {
        <- Events.emit(UserJoinedRoom { roomId: id, user: u })
        let existing <- connections.values.filter(c => c != conn).collect
        <- existing.parTraverse(c => c.send(UserJoined(u)))
      }
    }

    on leave(u: UserId, _conn: Connection[ServerFrame]) given Events {
      <- connections.remove(u)
      <- members.remove(u)
      <- Events.emit(UserLeftRoom { roomId: id, user: u })

      let remaining <- connections.values.collect
      <- remaining.parTraverse(c => c.send(UserLeft(u)))
    }

    on post(sender: UserId, content: Text, clientMsgId: ClientMessageId)
        given Clock, Events, Idempotency {
      <- Idempotency.dedup(on: (sender, clientMsgId), expiresAfter: 5.minutes)?
      let now <- Clock.now()
      let msg = Message {
        id:      MessageId.fresh(),
        roomId:  id,
        sender:  sender,
        content: content,
        sentAt:  now,
      }

      <- history.append(msg)

      let conns <- connections.values.collect
      <- conns.parTraverse(c => c.send(MessageBroadcast(msg)))

      <- Events.emit(MessageSent {
        roomId:  id,
        sender:  sender,
        content: content,
        sentAt:  msg.sentAt,
      })
    }
  }
}

context chat.notifications {
  consumes chat.shared
  consumes chat.rooms

  service OnMessageMention
        from Events(chat.rooms.MessageSent)
        given Push: PushService, Idempotency {
    on event(e: chat.rooms.MessageSent) {
      <- Idempotency.dedup(on: env.eventId, expiresAfter: 1.day)?
      let mentions = parseMentions(e.content)
      <- mentions.parTraverse(userId =>
        Push.notify(userId, "Mentioned in chat", e.content)
      )
    }
  }
}
```

The `WebSocket(in: ClientFrame, out: ServerFrame)` binding parameterises the typed message vocabulary at the service declaration, making the contract bidirectional and static. The `connection` identifier inside each handler refers to the connection that produced the frame; it is a `Connection[ServerFrame]` value with identity and lifecycle. On `open`, the gateway routes the connection into the `Room` agent, where it becomes part of the agent's typed state alongside `members` and `history`. The platform's hibernatable-WebSocket support ensures the connection survives the agent's hibernation: when the Room sleeps between bursts of activity, the connections remain held in storage; when traffic resumes, the same `Connection[ServerFrame]` values are still present and ready to receive broadcasts.

The `Room.post` handler shows three different effect kinds in one domain operation, each appropriate to what its channel can deliver. *Append-to-log* is a storage write that commits with the handler — atomic with the rest of the handler's effects. *Broadcast to connections* is a parallel issuing of at-most-once WebSocket sends; the runtime reports delivery failures asynchronously via the `on close` handler rather than at the call site, matching the protocol's reality. *Event emission* is a cross-context outbound effect with at-least-once delivery, released at commit, available to any subscriber in any context. Three different reliability tiers, one handler body, each visible at the call site.

The `Idempotency.dedup` call at the start of `Room.post` makes the post handler safe against client retries. If a network hiccup causes the client to resend a `Send` frame with the same `clientMsgId` within the retention window, the provider returns the cached outcome without re-executing — no duplicate broadcast, no duplicate log entry, no duplicate event. The client supplies a stable identifier; the `Idempotency` capability handles deduplication.

`chat.notifications` shows the cross-context event subscriber pattern. `MessageSent` is emitted by `Room` in `chat.rooms` and consumed by a service in `chat.notifications`. The subscriber is itself a service (the "subscribers are services" rule from Section 7) that may then route to whatever capability handles the actual notification dispatch. Its own `Idempotency.dedup(on: env.eventId, ...)` call handles at-least-once event delivery: a re-delivered event will be deduped before the handler body runs.

Two architectural facts worth pulling out. *Per-publisher FIFO* means messages from a single user — via that user's single `Send` frame stream — arrive at the `Room` in the order they were sent; messages from different users have no global ordering guarantee. This is exactly right for a chat: within one user's stream, order is essential; between users, simultaneity is real. *Connections as typed agent state* means `Connection[ServerFrame]` parameterises what the server can send through that connection; an attempt to send a frame of the wrong shape is a compile-time error at the `c.send(...)` site, not a runtime mystery on the wire. The channel itself is typed.

The example simplifies in one place worth flagging: one connection per user. A production version would handle multiple connections per user (multiple devices, browser tabs) by storing `Map[UserId, Set[Connection[ServerFrame]]]` and tracking per-connection liveness. The pedagogical version sacrifices that detail to keep the agent body readable while still exercising the architectural primitives the example exists to demonstrate.

### Example 3: Order placement with a durable Sagas provider

Example 1 used the `Sagas` capability with `compensate(action)` for in-handler compensation. The in-memory provider runs registered compensations on handler abort within the same handler invocation — right for most cases. For systems where the cost of a lost compensation is unacceptable (payment authorised, no order placed, no refund issued — left inconsistent until external reconciliation), the same `Sagas` capability accepts a *durable provider* that persists compensation records and recovers across crashes. The handler shape is unchanged; only the provider binding differs.

For richer durable workflows that need explicit forward/undo step pairs as serialisable descriptors (rather than closures), the framework can additionally expose a structured `Workflows` capability with a different operation shape:

```
context bynk.workflows {

  type WorkflowKey = String
  type WorkflowError = enum { Aborted, RecoveryFailed }
  type WorkflowContext  -- opaque; provided by the framework

  capability Workflows {
    run[T](
      name: String,
      key:  WorkflowKey,
      body: WorkflowContext -> Effect[Result[T, WorkflowError]]
    ) -> Effect[Result[T, WorkflowError]]
  }

  -- methods on WorkflowContext (the framework's handle for an open workflow)
  fn WorkflowContext.step[A, E](
    self,
    name:    String,
    forward: () -> Effect[Result[A, E]],
    undo:    (A) -> Effect[Unit]
  ) -> Effect[Result[A, E]]

  exports transparent { WorkflowKey, WorkflowError }
  exports opaque      { WorkflowContext }
  provides            { Workflows }
}
```

The order handler, rewritten to use the structured workflow surface:

```
on place(u: UserId, c: Cart) -> Result[Receipt, OrderError]
    given Inventories, Payments, Fulfilments, Workflows {

  validateCart(c)?
  user := Some(u)
  cart := Some(c)

  Workflows.run("place-order", WorkflowKey.forOrder(id)) { wf =>

    let reservations <- wf.step(
      name    = "reserve",
      forward = () => reserveAll(c.items),
      undo    = rs => rs.parTraverse((sku, rid) => Inventories(sku).release(rid))
    )?

    let authId <- wf.step(
      name    = "authorise",
      forward = () => Payments.authorise(c.total, u),
      undo    = aid => Payments.refund(aid)
    )?

    let shipId = ShipmentId.forOrder(id)
    <- wf.step(
      name    = "schedule",
      forward = () => Fulfilments(shipId).schedule(lineItems(c, reservations), u),
      undo    = _ => Fulfilments(shipId).cancel()
    )?

    status := Placed
    Ok(Receipt { user: u, total: c.total, shipment: shipId })
  }
}
```

The visible differences from Example 1 are localised: each compensable step is explicit, with its forward and undo as paired arguments; the workflow's body returns a `Result[T, WorkflowError]` that propagates through `?` like any other Result; the `Workflows` capability appears in the `given` clause alongside the others.

What the framework provides behind the capability surface:

- *Workflow record creation.* On entry to `Workflows.run`, the framework writes a durable record indexed by `WorkflowKey` containing the workflow's name, state (`InProgress`), empty step list, and start timestamp. This commits before the body runs.
- *Step persistence.* Each `wf.step(...)` call writes a record before invoking the forward, updates it with the forward's result, and atomically records the undo for later replay. The undo is captured as a serializable call descriptor — agent ref, method name, captured arguments — not as an opaque closure. The framework's API enforces this serializability at the step boundary.
- *Forward execution.* The framework invokes the forward via Bynk's normal cross-agent call machinery. The forward is just a Bynk expression; the framework records only its result.
- *Unwind on `Err`.* When the body returns `Err` or propagates one through `?`, the framework runs each recorded undo in reverse order via at-least-once delivery. Receivers' `Idempotency` capability calls handle replay correctness.
- *Crash recovery.* On agent restart, the framework's recovery process scans workflow records for status `InProgress` with no live executor (detected via heartbeat absence or restart marker). For each, it transitions to `Recovering` and dispatches remaining undos. The recovery process is itself idempotent against the workflow record — it can crash and resume.
- *Completion.* On `Ok` from the body, the framework marks the workflow `Completed` and the record becomes eligible for cleanup (immediately or after a retention window for audit).

The implementation has choices. A Cloudflare-native version stores workflow records in a dedicated `WorkflowStore` agent in `bynk.workflows` and uses Cloudflare Queues for delayed undo dispatch. A Temporal-backed version delegates: `Workflows.run` wraps a Temporal workflow whose activities are the forward and undo pairs. A test-mode version provides an in-memory implementation that runs forwards and undos without persistence, useful for unit tests of handlers that use workflows without the workflow infrastructure being spun up. From the application's perspective, the workflow body is unchanged.

The contrast with Example 1 is the point. Example 1 uses `Sagas.compensate(...)` for in-handler compensation — cheaper at runtime (no extra durable writes), shorter in source, and adequate for chat, internal tools, retryable workflows, and anything where a 15-minute reservation expiry or operator reconciliation is an acceptable safety net. Example 3 uses the structured `Workflows` capability where the compensation must run regardless of crashes; it pays the durability cost at the step API surface (one extra durable write per step), and the handler explicitly threads `Workflows` through its `given` clause to signal that durable workflow semantics are in play. The decision between the two is architectural and visible at the call site — exactly the principle the rest of the language is built on.

## 21. Open Decisions

The following are unresolved and will need attention in subsequent sessions.

- **Authentication scheme extensibility.** The initial vocabulary of schemes (bearer token, HMAC signature, mTLS, none, internal) covers common cases. Whether to support user-defined schemes — for example, by exposing a `Verifier[T]` capability that custom actor declarations can plug into — is open. The conservative starting position is a closed set that can be opened later. _Partially resolved (v0.151, ADR 0175):_ the set now **opens by widening the enum**, starting with a compiler-generated `Oidc` (JWKS/RS256+ES256) scheme — chosen over a user-supplied `Verifier[T]` because a user verifier reintroduces the hand-written-crypto footgun ADR 0085 removed, and OIDC is a standard the compiler can own totally. A genuinely user-pluggable `Verifier[T]` remains open for schemes the compiler cannot standardise (bespoke session cookies, opaque-token introspection).
- **Protocol extensibility.** Structurally similar to authentication scheme extensibility: whether the set of protocols (HTTP, Queue, Cron, Alarm, WebSocket) is closed or open to user-defined additions. Both questions concern platform-supplied versus user-extensible contract vocabularies, and both can take the same conservative starting position — a closed initial set, opened later if a need emerges. Session-typed protocols (describing the temporal order of messages, not just static handler shape) are a related extension worth considering once the type system is settled.
- **Bounded context syntax and granularity.** The shape of bounded contexts is settled (Section 8): the architectural primitive at the organisational layer, wrapping actors, services, agents, types, and capabilities, exporting contracts not implementations. Concrete syntax for declaring a context, expressing its imports and exports, and naming types across contexts is open. So is the typical granularity — one application as a single context vs many small contexts — which is probably a question of practice rather than language.
- **Error handling.** Replaced by the failure model in Section 13. The deferred sub-questions are listed below as separate items.
- **The catalogue of first-party capabilities is open.** The first committed first-party capability is `Sagas` for durable workflow compensation, sketched in Section 18 and demonstrated in Section 20. Others may follow as the language matures: retry policies with backoff, structured rate limiting, distributed tracing instrumentation, schema migration helpers, batch processing patterns, structured caching, scheduled-task orchestration. Each is a candidate for first-party status if (a) the problem is common to many Bynk applications and (b) a clean capability surface exists that doesn't require the language to grow. The criteria for first-party status — what the Bynk project supplies as the canonical answer versus what's left to the community — is itself a design question worth thinking about as candidates emerge. The initial set is `Sagas`; others will be added deliberately rather than by accretion.
- **Localisation (i18n) is Bynk's first open surface for user-facing text, and it opens through the platform-capability mechanism, not a new one.** Every built-in error today (`ValidationError`) carries a free-text English string with no seam for turning it into another language. The proposed shape — an ambient `Locale` capability (`current() -> Effect[LocaleTag]`, the same zero-argument ambient-read shape as `Clock.now()`) paired with a pure, total `render(tag, msg) -> String` — needs no new language mechanism: it is an ordinary platform capability plus an ordinary pure function, following the `Clock`/`Secrets` precedent (Section 18) exactly. What's open: the exact `Message` descriptor shape and whether it is produced automatically from refinement failures (a separate, not-yet-scoped `predicate`-declaration language change) or only ever constructed by hand; where locale negotiation lives (`Accept-Language` resolved at the capability seam, versus threaded explicitly); missing-translation fallback semantics; and whether to adopt ICU MessageFormat (plurals, gender, locale-aware number/date/currency) as the bundle format given `Money`/`Instant`/`Duration` will eventually want locale-aware rendering too. Tracked under spine issue [#838](https://github.com/accuser/bynk/issues/838),
now retired — see
[`design/archive/retired-tracks.md`](archive/retired-tracks.md) — with
`Locale`/`LocaleTag`/`Message`/`render` shipped (ADR 0256), real
`Accept-Language` negotiation shipped (ADR 0277), and ICU MessageFormat
shipped as part of the sibling message-bundles track (ADR 0276). The
automatic-from-refinement-failures half named above did not ship: it depends
on the `predicate`-declaration language change, which remains unfiled.
- **The precise refinement vocabulary at type declarations is open.** The committed shape is refinement-at-declaration with a fixed vocabulary of data predicates (Section 15). The initial set sketched: `Matches(regex)`, `InRange(min, max)`, `MaxLength(N)`, `MinLength(N)`, `Length(N)`, `NonNegative`, `Positive`, `NonEmpty`. What's open: the exact predicate list and naming for v1; the boundary between "predicates the compiler checks structurally" and "predicates that require evaluation against arbitrary values" (the latter being kept narrow to avoid SMT-solver territory); whether composite refinements (e.g., `Pattern("address-{[A-Z]{3}}-{NNNN}")` for structured identifiers) deserve dedicated syntax. The discipline is to start with a small set, evolve slowly, and resist user-extensible predicate languages. (`Scale(N)` was considered and rejected — it solved a problem that better primitive choice solves more cleanly; see the discipline note in Section 15.) Code refinements (`Pure`, `Total`, `Deterministic` on functions; `ReadOnly`, `Idempotent` on capability operations) form a smaller secondary surface — most are compiler-inferred, with explicit annotation reserved for contractual commitment; the precise rules for inference vs annotation are themselves part of this question.
- **Declarative supervision.** Beyond the platform's built-in restart semantics, whether the language should support declared supervision relationships (escalation policies, restart strategies, child specifications) is open. Likely a future addition rather than part of the floor.
- **Outcome chaining sugar.** `Result`-typed sequences become verbose under deep matching. A `?` propagator (Rust-style), do-notation, or block forms are desirable but are sugar over the foundation rather than part of it. The form to commit to is open.
- **Type system extensions beyond the committed core.** The core type system is committed (Section 15): Hindley-Milner with closed sums and nominal records, opaque types for visibility, capability interfaces for effects, generic types for storage and held resources, constrained refinement for event patterns and invariants. What remains open: whether to generalise refinement beyond the constrained instances (driven by accumulated need); whether to add a constrained form of row polymorphism for capability composition; whether session types eventually earn their keep for WebSocket and saga protocols; whether higher-kinded types prove necessary for library abstractions; whether capabilities can compose hierarchically (`PostgresGateway` extending `SqlGateway`) or remain flat. The starting position is conservative — add only what the architecture demonstrably needs — with each of these as candidate extensions if pressure emerges.
- **Storage type primitive set.** The current set is a working hypothesis. CRDT-style counters and time-windowed aggregates are candidates for either additional primitives or library types. (Pub-sub fan-out, previously listed here, is handled by the Events service protocol — Section 7.) Whether `Held[T]` becomes a kind with multiple concrete instances beyond `Connection[F]` will be answered as the platform capability surface grows.
- **Events surface — pattern syntax and remaining details.** Events are committed as a service protocol with at-least-once delivery, per-publisher ordering, type-as-topic routing, pattern-based subscription refinement on payload and envelope (Sections 7), envelope-mediated runtime metadata, isolated subscribers, schema-versioned envelope with `via schema(...)` dispatch, additive evolution via field defaults, replay through default-driven upgrade on read. What remains open: concrete syntax for event declarations; the boundary between simple structural patterns (clearly in scope) and computed predicates (which start to look like SQL where-clauses and may belong in the handler body rather than the subscription); in-process delivery optimisation when publisher and subscriber are co-located.
- **Static satisfaction checking for invariants.** All invariants are runtime-checked at handler commit, with statically-provable violations flagged as compile-time errors (Section 14). The stronger property — *static proof of satisfaction*, where the compiler verifies that an invariant always holds rather than just that it never provably fails — remains open as a future enhancement. SMT-style verification or refinement type machinery could be added if pressure emerges; the current position is conservative.
- **Query algebra.** The minimum coherent set of operations on storage queries; how the algebra interacts with closures that capture agent state mid-query.
- **Compilation target details.** JavaScript versus WebAssembly for the first target; binding interop strategy; ahead-of-time vs just-in-time compilation.
- **Remaining syntactic choices.** The core syntax is committed (Section 16): glyph set, block structure, lexical conventions, parser tractability, the uniformity principle (one canonical way per thing, pipe operator and custom operators excluded). Documentation and comment forms are committed (Section 17). The invariant-predicate and test surfaces, committed only in shape when this was written, have since been designed and shipped as the one-predicate-surface testing track (Section 14); nothing in this area remains open.

Resolved (recorded for the historical record):

- **Tests are a `suite` declaration, not a third context kind — the one-predicate-surface model.** *This supersedes an earlier resolution* (recorded at the end of this bullet) that made tests a third top-level `test QualifiedName { ... }` kind. The shipped testing track (Section 14; decision records 0144–0155) replaced that wholesale: a `suite` names its target and is an ordinary top-level declaration legal in any file, and test-ness is a property of the declaration — the build strips it, the runner runs it — not a distinct kind. `expect` is the one assertion, over the same predicate surface as `invariant`/`ensures`/`transition` (no separate matcher vocabulary); `Val[T]` replaced `Mock[T]` as the value fabricator; per-seam `provides Cap.method(pattern) returns value | fails` (a value or fault, never a re-implementation) replaced both the inline `provides` bodies and the `mocks` block, removing the word "mock" from the language; the tier dial `as unit | integration | system` replaced `#tag` filtering and the separate `integration` form; and interaction is asserted over auto-recorded observation (`expect Cap.op called …`, `trace`) rather than a `.calls` list. *The superseded resolution, for the record:* tests were to be declared as `test QualifiedName { ... }`, a third kind alongside `context` and `commons`, granting bounded privileges over an explicit target — direct construction of the target's types, white-box access to its private items, and capability substitution — with `Mock[T]` as the fabricator for foreign types and `test commons` for shared fixtures. That model was set aside because a separate declaration kind and its white-box privileges proved heavier than the goal warranted; the honest extension of the language's own principle (it already gates assertions at the block, not the path) is that test-ness rides on the `suite` declaration.
- **Commons mix in; contexts export. These are different mechanisms for different architectural needs.** A foundational architectural distinction (§2.1.3, §2.1.4 of type system spec, Section 8 of this document). *Mixin* (commons) shares vocabulary into a context's local language: `uses commerce.money` brings declarations into the using context's scope as if locally declared. Commons declarations don't have visibility levels because there is no boundary to cross — the declarations *become* part of the using context's language. *Exports* (context) govern the contract a context offers to callers of its services: opaque exports give callers tokens (no readable structure); transparent exports give callers readable data (inspectable but not constructible); private declarations stay inside. A context typically uses both — mixing in commons for shared vocabulary, exporting selectively for its service-boundary contract. The two questions are different and warrant different mechanisms: "what vocabulary should be part of my context's language?" → mixin; "what types should callers of my services interact with?" → exports.
- **Commons are the canonical construct for shared vocabulary across bounded contexts; context-owned types are sealed against external construction.** Together these form a two-part commitment about types and boundaries (§2.1.3, §2.1.4 of type system spec, Section 8 of this document). *First*, full encapsulation: a value of a type defined in a context can only be constructed within that context. Export visibility (opaque or transparent) governs the read side — what consumers can see of the type's structure — but never grants construction authority. Opaque export makes a type a token outside its context (holdable but not inspectable); transparent export makes it readable data outside (inspectable but still not constructible). Construction of a context-owned type from outside its defining context is a compile error. The architectural consequence: cross-context interaction is through service operations (handlers in the owning context, invoked by peers) and events (published from inside, received outside as facts). External "factory functions" like `Voucher.of(...)` are also forbidden — they would be construction by another name, smuggling past encapsulation. The idiomatic Anti-Corruption Layer pattern (pattern-match foreign error, construct local error) is fully consistent with the rule and is the *only* shape cross-context error translation can take. *Second*, commons are mixed in rather than imported: when a context says `uses commerce.money`, the commons's declarations are brought into the using context's scope as if locally declared. Each using context has its own nominal type derived from the mixed-in declaration; two contexts mixing in the same commons have structurally identical but nominally distinct types. Construction is admitted in each using context (the type is local). Cross-context value flow goes through structural projection at the boundary: data crosses the wire; the receiving context constructs its own nominal type from that data, applying its local refinements. The commons construct itself has a compiler-enforced constraint set (no agents, services, capabilities, storage, providers, given-clauses-in-functions); a separate import keyword `uses` (distinct from context-to-context `consumes`); permitted cycles among commons (no runtime coupling); a flat, organisational naming hierarchy (a commons named `commerce.money` is not architecturally "for" `commerce.*` contexts); and source-level mixin compilation rather than separate compiled artefacts (commons are project-source code, vendored rather than packaged). The split between commons (compile-time vocabulary, mixin-incorporated, no behaviour) and capabilities (runtime contracts with providers, behavioural) is orthogonal: capabilities answer "how does my code talk to the outside?", commons answer "what does our code talk about?"; a context typically declares both kinds of import at its top. The earlier worked-example convention of putting shared types in a "shared" context (`commerce.shared`) is rejected; the earlier softer encapsulation that permitted transparent construction is also rejected; the earlier model of commons as separately-compiled-and-imported modules is also rejected in favour of source-level mixin.
- **The language is named Bynk.** Cornish for a cairn — a built-up rocky landmark on the moors. The architectural metaphor maps exactly: applications are bynks, built from small individually-meaningful primitives stacked into coherent structures that endure and orient the traveller across difficult terrain. The name is short, pronounceable, distinctively Cornish, and not heavily branded in software. Source files use the `.bynk` extension. Pronounced as English would suggest from the spelling — one syllable, "bynk." *Search-term note*: "Bynk" alone is well-occupied by Magic: The Gathering's Bynk planeswalker character, which has a large and active SEO presence. For discovery purposes the project disambiguates with "bynk-lang" or "Bynk programming language" in titles, repository names, and domain choice; the typical naming-collision workaround that Rust, Go, and Swift have all navigated. Eyes-open trade-off; not a fix to make.
- **The language core stays minimal; first-party capabilities are the canonical extension surface.** What sits in the language: bounded contexts, services, agents, value types, handlers, storage primitives, capabilities as a mechanism, the query algebra, the failure model, the type system core, the documentation system. What's deliberately *not* in the language and is solved by capabilities instead: durable workflow orchestration, retry policies with backoff, structured rate limiting, schema migration helpers, distributed tracing, batch processing patterns, and similar cross-cutting concerns. *First-party capabilities* are those the Bynk project itself supplies as canonical answers to common architectural concerns; they live in framework contexts (`bynk.sagas`, etc.) that application contexts consume; their implementations are ordinary Bynk code; their surfaces are ordinary `given` clauses; there is no special compiler support and no syntactic distinction between using a first-party capability and using one written by an application team or a third party (Section 18). The benefit: the language stays learnable, the cost surface for harder problems lives in capabilities where they can evolve without language churn, and the extension story is uniform across first-party and third-party. The cost: some problems with elegant language-level solutions are solved through a slightly more verbose capability surface instead. The trade is deliberate.
- **Durable workflow / saga semantics are framework concerns, integrated through the capability mechanism, not language features.** Compensation in Bynk flows through the `Sagas` capability (Section 13) with provider variants for in-memory (lost on agent runtime crash, right for most handlers) and durable (registrations persist across crashes, right for long-lived workflows) semantics. For richer durable workflows that need explicit forward/undo step pairs as serialisable descriptors rather than closures, the framework can additionally expose a structured `Workflows` capability (Section 20, Example 3) with provider implementations including a Cloudflare-native version, Temporal-backed delegation, and in-memory test mode. Both `Sagas` and `Workflows` flow through the existing capability mechanism, are declared in handler `given` clauses, and can be substituted at the capability surface. This commits Bynk to a deliberate scope: durable workflow orchestration is not in the language and never will be; it's a capability concern, paid for in capability surface area only by handlers that need it. The earlier sketch of a `durable on abort:` language extension is explicitly rejected in favour of this approach.
- **Refinement lives on type declarations as its primary mode, with the same declaration doing triple duty.** A refined type definition (`type VoucherCode = String where Matches("[A-Z0-9]{8}")`) provides compile-time type identity, runtime validation logic, and external schema specification from a single source — collapsing what conventional ecosystems split across types, validation libraries, and schema generators into one declaration (Section 15). The refinement vocabulary is constrained to *data predicates* that can do all three jobs: statically checkable, runtime-executable, serialisable to external schemas. Initial vocabulary is small (`Matches`, `InRange`, `MaxLength`, `MinLength`, `Length`, `NonNegative`, `Positive`, `NonEmpty`); growth is deliberate and slow. Refinement points: value type declarations (primary), event subscription patterns, agent invariants, actor authorisation invariants (anticipated), capability operation refinements (framework-internal). Use-site refinement on parameters and return types was considered and rejected for verbosity; the discipline is "refine to create types worth naming, reuse the named types everywhere." Function purity, totality, and determinism are compiler-inferred from the function's surface; explicit annotation is reserved for cases where the contract should be committed. **Refinement complements primitive choice rather than substituting for it**: where a sharper primitive would make a constraint structural, choose the primitive (the canonical instance is money-as-integer-minor-units rather than money-as-decimal-with-Scale-predicate). The unification of types, validators, and schemas is the architectural payoff; the constrained vocabulary keeps the compiler bounded and the diagnostic experience consistent.
- **All storage operations are Effect-typed.** Operations on `Cell`, `Map`, `Set`, `Log`, `Queue`, `Cache`, and `Ref[A]` method calls return `Effect[T]` and require `<-` to await (Section 10). This makes storage cost visible at the call site, mirrors the discipline applied to cross-context calls, and remains honest across compilation targets where storage may be genuinely async. The single ergonomic exception is `Cell`: implicit dereference (in value position) and `:=` assignment are syntactic sugar over the Effect-typed read and write operations — the compiler inserts the await automatically for these single-value idioms. All other storage types have no sugar; every operation site is an explicit `<-`. In-memory (sync, non-durable) storage types are deliberately deferred from v1; the agent's `store` fields are durable, and local `let` bindings within handlers are the only sync state. A future `Local[T]` or similar kind for in-memory caches can be added if pressure emerges.
- The actor / service / agent vocabulary is settled.
- Per-sender FIFO is the floor for cross-agent ordering; explicit awaits (in whatever syntactic form) mark causal cuts in transitive cases.
- Handlers are uniform under invocation source.
- Actor declarations carry identity, authentication scheme, authorisation invariants, idempotency expectations, and replay/ordering assumptions; services consume them via handler clauses.
- Service composition follows a uniform pattern across protocols (HTTP, Queue, Cron, Alarm, WebSocket); long-lived runtime resources flow from services to agents at the moment of acceptance.
- Failure is divided into outcomes (typed values, part of the handler's contract) and faults (untyped runtime events that abort the handler atomically). The two kinds are not on a spectrum and the language refuses to conflate them.
- Atomic-handler semantics extend beyond storage commits to outbound message release: a handler's effects are all-or-nothing.
- `attempt` / `recover` is the only construct that converts a fault into an outcome, and is intended for deliberate use.
- Multi-step domain workflows belong in agent handlers, not at the service layer. Services do shape validation (parsing typed messages from the wire), authentication (via actor types), single-call dispatch, and response shaping; everything else belongs in agents where per-handler atomicity is available.
- Agents may decompose handler logic via private `fn` helpers that declare capabilities and access the agent's storage. These helpers share the calling handler's atomic transaction, allowing readable decomposition without introducing new commit boundaries.
- Effectful iteration is handled by `traverse` (sequential, FIFO-preserving) and `parTraverse` (opportunistic concurrency, results in input order). Both apply to in-memory collections. For Result-returning functions, both default to short-circuit-on-first-Err semantics; `traverseAll` and `parTraverseAll` are the explicit collect-all variants for cases where domain failures are information the caller wants to gather. Pure list operations (`map`, `filter`, `collect`, `partition`, `fold`) compose with these to handle outcome partitioning and aggregation.
- The architectural primitive at the organisational layer is the **bounded context** (DDD sense), not a module or package. A bounded context wraps a coherent set of actors, services, agents, value types, helpers, and consumed/provided capabilities. It exports contracts, not implementations; cross-context references go through the contract surface; the language enforces context boundaries the same way it enforces the actor/service/agent split. Platform-supplied capabilities (`Clock`, `Random`, `Http`, etc.) live in platform contexts that application contexts consume.
- Actors are *relative to a context*, not absolute. Cross-context callers play the actor role from the called context's perspective, using `auth = Internal` and statically-verified contracts. The same actor-as-contract mechanism unifies external boundary and cross-context boundary; the difference is the auth scheme and the timing of verification (runtime for external, static for cross-context). This makes anti-corruption structural: a context can only be reached through its declared exports by its declared actors.
- Exported types carry a *visibility*: **opaque** (name published, structure hidden; held and passed but not introspected), **transparent** (structure published; freely constructed and pattern-matched), or **private** (not exported). Domain identifiers are typically opaque; shared value vocabulary is transparent; internals are private. This resolves the shared-types tension structurally — a context's identifier means something only inside that context, and outside it is an opaque token; genuinely shared value types are a small, deliberately transparent set. Translation at boundaries is concentrated in the type's owning context as its parse/serialise exports, rather than being a separate anti-corruption-layer concern.
- **Held runtime resources** are a kind in the type system alongside values and `Ref[A]`. `Connection[F]` is the first concrete instance — a typed handle to a WebSocket connection, parameterised by the server-frame type, with lifecycle managed by the platform (including transparent survival of agent hibernation). The general pattern is `Held[T]` for any typed handle whose lifecycle the runtime owns. Held resources can be passed as typed message arguments and stored in agent state, but are not value-typed.
- **Cross-context interaction has two semantic shapes**: typed *commands* (contract-mediated, targeted at a specific agent in another context, imperative verbs) and typed *events* (service-mediated via the Events protocol, broadcast past-tense facts to zero-or-more subscribers). Both reuse the existing service-and-actor-and-protocol machinery; the choice between them is a domain-modelling decision about coordination versus announcement.
- **Delivery semantics** are at-least-once for cross-agent commands and events, at-most-once for WebSocket sends. The atomic-handler invariant determines when outbound effects release; the runtime determines what happens after release. At-least-once is made safe by construction through the `Idempotency` capability: a handler that calls `Idempotency.dedup(on: key, expiresAfter: duration)` near the start of its body dedupes against the named key for the retention window, atomically with the handler's other commits, so re-delivery returns the cached outcome without re-executing the body. Receivers that cannot be expressed idempotently accept an idempotency key from the caller and dedupe against it through the same mechanism.
- **Validation has a layered, fully-specified surface.** The architectural cleanness (capability injection, atomic handlers, bounded-context boundaries) makes code testable by construction. On top of that:
  - **Tests are `suite`/`case`/`property` declarations over one predicate surface.** A `suite` names its target and holds `case`s (supplied subjects) and `property`s (generated ones); `expect <bool>` is the single assertion, reusing the same predicate surface (`is`, `implies`, operators, pure methods) as `invariant`/`ensures`/`transition`, with no separate matcher vocabulary. Value fabrication is `Val[T]`; per-seam test doubles are `provides Cap.method(pattern) returns value | fails`; the tier dial `as unit | integration | system` sets how much of the real world runs; interaction is asserted over auto-recorded calls (`expect Cap.op called …`, `trace(Cap.op)`). Suites are stripped from the deploy build and run by `bynkc test`. *(Supersedes the earlier `test QualifiedName` / `Mock[T]` / `assert` / `.calls` sketch.)*
  - **Invariants and transitions on agents** are the commit-boundary half. Invariants are universally-quantified properties with a small predicate language: ordinary expressions plus `implies` (directional logical implication), `is` (pattern matching as Boolean), and implicit dereference of `Cell` in expression position. A `transition` widens the subject to the `old`/`new` step. Both are runtime-checked at handler commit (a `transition` from the second commit onward; intermediate handler states are unconstrained); statically-provable violations are flagged as compile-time errors. Per-agent scope only; cross-agent invariants are kept at library / saga / scenario level. Static proof of satisfaction remains a future enhancement.
  - **Generative testing is first-class, not library-level.** `property`/`for all` generates values over refinement domains, function `requires`/`ensures` contracts are attacked by the runner over generated arguments, and `for all run: History[Agent]` generates and drives whole call-histories through the real handlers — all seeded, shrinking, and reproducible. Cross-agent scenarios remain a library / `system`-tier concern. Supplied cases describe behaviour existentially; generated properties and histories describe it over ranges; invariants and transitions describe contracts universally; together they cover the validation space.
- **Services are the bounded context's platform interface; agents are the domain primitive.** Every typed message arriving at a context enters through a service — HTTP requests, queue messages, cron triggers, alarms, WebSocket frames, and published events are the same primitive on the receiving side, differing only in protocol. Agents hold state and produce domain effects: state changes, event emissions, cross-agent calls, capability use. The platform sees none of the latter directly; it sees only protocol responses (returned by services) and outbound effects (intercepted via capabilities and event emission). This is one principle, not several, and it makes "authenticate at the boundary," "anti-corruption layer," "ports and adapters," and "command-event separation" structural facts of the architecture rather than patterns the developer must remember.
- **Events have a fuller surface.** Declared with the `event` keyword in the owning context as transparent value-types (opaque fields permitted); emitted via the platform `Events` capability with release at handler commit (atomic-handler invariant applies); routed by type-as-topic (event type is the topic, no explicit topic names); ordered per-publishing-agent (the same per-sender FIFO floor as cross-agent commands); carrying runtime-managed envelope metadata (event ID, publisher ID, timestamp, schema version) for idempotency, diagnostics, and version dispatch; delivered with at-least-once semantics deduped via the `Idempotency` capability keyed on `env.eventId`; consumed by services (never directly by agents — agents are routed to by the subscriber service), with subscriber failure isolated.
- **Event versioning is the existing refinement mechanism extended to the envelope.** Pattern-based subscription refinement extends from payload to envelope via a new `via <field>(pattern)` clause, with `via schema(...)` committed for v1 and other envelope-pattern extensions composing without grammar changes. Schema evolution is additive in the common case through default expressions on record fields (`field: Type = expr`), evaluated at deserialisation when the field is absent from the wire format. The compiler maintains a schema registry across builds and emits a schema-evolution report on change; explicit `@schema(N)` annotations on event types are available for teams that want to pin versions. Replay falls out of the same mechanism: a new subscriber backfilling from log history reads events in their original wire format, the runtime upgrades them to the current schema using declared defaults, and subscribers see them as if just emitted. Breaking changes (renames, type-narrowing, semantic redefinitions) are handled by convention — introduce a new event type with a versioned name, emit both during transition, retire the old type — rather than by language feature.
- **Pattern-based subscription refinement.** Event subscriptions may carry a structural pattern matched against the event type. The compiler type-checks the pattern; the runtime enforces it before the handler runs; the handler body assumes the filter has passed. This mirrors auth dispatch on services (filter in the signature, verified before the body, body assumes the matched variant) and reuses the same architectural moves, avoiding event-type proliferation while keeping filtering declarative and visible. Simple structural patterns are in scope; computed predicates are deferred to in-handler filtering for now. This is a constrained instance of refinement types confined to event subscription; general refinement types in the type system remain in Open Decisions.
- **Cross-cutting concerns are capabilities; the language has no syntactic sugar that hides capability usage.** A foundational architectural commitment: behaviours that touch the world (compensation, idempotency, tracing, metrics, retries, rate limiting, audit logging) are expressed as capabilities the handler declares in `given` and invokes through explicit calls. The language deliberately rejects sugar that would abstract over capability use — `on abort:` and `idempotent on` clauses are *not* in the language. A developer who needs compensation declares `given Sagas` and calls `Sagas.compensate(...)`; a developer who needs deduplication declares `given Idempotency` and calls `Idempotency.dedup(...)`. The capability dependency is visible in the handler signature; the operation is visible at the call site; nothing about how the handler behaves can be hidden behind syntax that doesn't mean what it appears to. This aligns with the broader Bynk principles: one canonical way per thing (sugar plus capability would be two ways), no hidden control flow (capability calls behave like every other awaited call), and pedagogical clarity (students learn capabilities deeply rather than the sugar that abstracts over them). The cost is verbosity in common cases — `<- Sagas.compensate(() => undo)?` is longer than `on abort: undo` — but the benefit is that every cross-cutting concern is structurally visible. The earlier commitments to `on abort:` as a language primitive and `idempotent on <expr> expires after <duration>` as a handler annotation are explicitly rejected.
- **The `Sagas` capability is the canonical compensation mechanism** (Section 13). A handler that needs compensating actions declares `given Sagas` and calls `Sagas.compensate(action)` to register an undo to run if the handler aborts. Registered actions run in LIFO order on abnormal exit (via `?` propagating Err, or via fault propagation), each wrapped in best-effort attempt by the provider. On normal exit, registrations are discarded. The capability has two provider variants: an in-memory provider (default, registrations live in handler-local state, lost on agent runtime crash — right for the common case) and a durable provider (registrations persist across crashes, right for long-lived workflows). For richer durable workflows that need explicit forward/undo step pairs as serialisable descriptors, the framework additionally exposes a structured `Workflows` capability (Section 20, Example 3) with a different operation surface. Both flow through the existing capability mechanism; both are visible in the handler's `given` clause.
- **The `Idempotency` capability is the canonical at-least-once safety mechanism** (Section 12). A handler that needs deduplication declares `given Idempotency` and calls `Idempotency.dedup(on: key, expiresAfter: duration)` near the start of its body. The provider records the call's eventual outcome atomically with the handler's other commits; on a subsequent invocation with the same key inside the retention window, the call returns the cached outcome without re-executing the rest of the handler body. The capability has the same provider-variant structure as Sagas: in-memory (handler-local, lost on restart, right for short-window cases) and durable (records survive crashes, right for canonical at-least-once safety). For event subscribers, the canonical key is the event envelope's identifier; for command handlers, the canonical key is a domain identifier supplied by the caller. The mechanism applies uniformly to all handler kinds — agent commands, service handlers, event subscribers — through one capability surface.
- **The compilation strategy is committed** (Section 19). The compiler emits typed TypeScript as its primary target, with a hand-written TypeScript runtime library providing the constructs that would be tedious to inline (sum-type discrimination, the `Result`/`Option` machinery, event-bus dispatch, capability resolution, default providers for the standard capabilities) and per-target platform bindings injecting the right runtime objects at link time. The output is debuggable, integrates with the Cloudflare TypeScript-first toolchain, and avoids the source-level-debugging cost that WASM-first compilation would impose. The compiler itself is written in Rust: a single-binary distribution, well-suited to the lexer-parser-typer-codegen pipeline, aligned with the broader tooling vocabulary. The pipeline has eight phases (parsing, name resolution, type inference, effect and capability analysis, architectural validation, lowering, code generation, bundling); each architectural primitive has a defined lowering (agents to DO classes, services to Worker handlers, events to the runtime bus, capabilities to injected objects). Cross-cutting concerns are not in the language and have no special lowering — they flow through as ordinary capability calls invoking provider implementations from the runtime library. The default deployment shape is one Worker bundle per bounded context, with the compiler generating each context's `wrangler.toml` from the source's bindings — DO bindings for agents, Service Bindings for `consumes` declarations, Queue and KV bindings for platform capabilities, all derived from the architecture rather than hand-written. Cross-context calls compile to invocations through the generated Service Bindings; monolithic deployment is a build-time option. Local development uses Cloudflare's open-source `workerd` runtime — the same engine that runs Workers in production — orchestrated by a single-command dev server that watches sources, recompiles incrementally, reloads `workerd`, streams logs, and runs affected tests on save. Production parity is structural: the same engine runs the same compiled output in development and production. Self-hosting and additional backends (WASM, native) are long-term and deferred.
- **The query algebra is its own surface** (Section 11). Storage types and in-memory collections share a uniform combinator vocabulary, with evaluation timing determined by receiver type: storage operations produce lazy `Query[T]` values that execute at terminals; in-memory operations are eager. Builders (`filter`, `map`, `flatMap`, `sortBy`, `take`, `skip`, `distinct`, `distinctBy`, `groupBy`, `join`, `joinOn`, `leftJoin`) construct queries; terminals (`collect`, `first`, `firstOrElse`, `count`, `fold`, `sum`/`min`/`max`/`average`, `any`, `all`, `forEach`) execute them. `Log[T]` adds time-window builders (`since`, `before`, `between`, `recent`, `reversed`) that use its implicit time index. Effectful iteration on in-memory `List[A]` adds `traverse` (sequential), `parTraverse` (concurrent), and the collect-all variants `traverseAll` and `parTraverseAll`; for Result-returning functions, the short forms short-circuit on the first `Err` by default, with the collect-all variants reserved for cases where domain failures should be gathered rather than abandoned. Queries are agent-local — cross-agent data access goes through message passing, not queries — preserving private agent state and structurally preventing distributed-query failure modes. Indexes are declared via storage refinement annotations (`@indexed(by: ...)`); the runtime maintains them as part of the atomic-commit machinery, and the compiler routes queries to indexes during query analysis with build-time warnings for missing or unused indexes. `Query[T]` is a first-class nameable type, supporting pure helpers that compose query fragments. Cost-based optimisation, materialised views, reactive queries, true async streaming iterators, time-travel queries, and SQL-like declarative syntax are explicitly deferred.
- **The type system is Hindley-Milner with architecturally-tuned extensions.** Closed sums and nominal records (not polymorphic variants or structural records); generic types for storage and held resources; opaque types implementing the visibility model (representation hidden externally, declared at the export clause); capability interfaces implementing the effects mechanism (named operation sets, declared not inferred, tracked through `given` clauses, sufficient without full type-class machinery); constrained refinement at three points (event subscription patterns, agent invariants, anticipated actor authorisation invariants). No subtyping; all polymorphism is parametric. Annotations are required at contract boundaries (functions, handlers, agent state, cross-context references, capability sets) and inferred internally (local lets, anonymous functions, generic instantiation where unique). Row polymorphism, session types, higher-kinded types, and type class hierarchies are not in scope initially — each is a candidate extension if the architecture demonstrably needs it. The type system's features map one-to-one onto the architectural commitments they support.
- **The surface syntax is committed in its core glyphs and lexical conventions.** Multi-character operators chosen for ligature aesthetics and parse tractability: `->` for return types, `<-` for awaits, `=>` for lambdas and match arms, `:=` for storage assignment, `==`/`!=`/`<=`/`>=` for comparisons, `&&`/`||`/`!` for logical operators, `?` for outcome short-circuit, `..` for rest-of-fields and ranges. Blocks delimited by braces; statements separated by newlines; semicolons accepted but rare. Generic parameters in square brackets. String interpolation as `"\(expr)"`. Numeric literals with `_` separators and `0x`/`0b`/`0o` prefixes, no type suffixes. A small reserved-keywords list. Identifier case used by the compiler to disambiguate module paths and constructor-versus-binding in patterns. Parser is LL(k) for small k.
- **Uniformity over alternative ways** is a guiding principle for surface syntax (Section 16): one canonical way to do each thing, on the pedagogical grounds that learners and reviewers benefit from a single idiom they recognise immediately. Two specific decisions follow. *Pipe operator (`|>`) is not in the language* — method chaining is the standard composition style, and a pipe operator would offer a second way to express the same idea without changing what's expressible. *Custom operators are not in the language* — the operator vocabulary is part of the core grammar rather than user-extensible, on the basis that named methods compose better with documentation, IDE tooling, and search than user-invented punctuation. Both decisions may be revisited as future enhancements if practice surfaces a strong case the principle would block, but the v1 default is no.
- **Documentation is first-class, attached to the architecture's declarations.** Doc blocks delimited by `---` at column zero, with markdown bodies; placed immediately after the declaration (first thing inside the body for body-having declarations, line-adjacent for inline declarations); H1 inferred from the declaration kind and name (overridable by writing `# Heading` as the first line); recognised `## Section` headers parsed structurally (Parameters, Returns, Errors, Example, Invariants, Dependencies, Lifecycle); wiki-style cross-references `[Name]` and `[Context.Name]` validated by the compiler against the declaration graph; code blocks marked `ex` executed as doctests with test capabilities by default; soft enforcement (warnings on undocumented public exports, configurable). The full comment story is three-level: `--` inline notes, `--- ... ---` documentation blocks, `{- -}` block comments. Tutorial-level prose in `doc/` files participates in the same cross-reference validation. Output is decoupled from source layout — the renderer walks the AST and emits docs in whatever order targets each consumer.

## 22. Influences

- Erlang / Elixir (actor model, FIFO ordering, fail-fast supervision)
- Roc (platform / application separation, capability effects)
- Unison (content-addressed, distribution-aware semantics)
- E (capability passing, promise pipelining)
- Pony (typed actors, reference capabilities)
- LINQ / Slick (query-as-value, lazy build / explicit execute)
- Smalltalk (everything-is-a-thing pedagogical clarity)
- DDD, Hexagonal Architecture, Clean Architecture (vocabulary, separations)
