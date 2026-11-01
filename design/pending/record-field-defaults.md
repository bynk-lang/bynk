---
level: minor
probes: emit_diagnostics, ts_writes
changelog: "A record type's fields may now declare defaults (`field: T = expr`), as an event's could. A record literal may leave a defaulted field out and takes its default, a spread still takes unwritten fields from its base, and a body decoded without the field's key (an HTTP body, a queue message, a service call, `Json.decode`, agent-state rehydration) takes it too. A default is a static value with a wire form, now including `Map.empty()` and `Set.empty()`; in a generic record it must hold for every instantiation. `FetchOptions`, `QueueSend`, `KvGet`, `KvPut` and `R2Put` give every field the default that reproduces the plain operation, so `Fetch.sendWith(req, FetchOptions {})` is `send(req)`. `bynk.event.bad_field_default` is now `bynk.types.bad_field_default`, `bynk.event.default_outside_event` is retired, and a default on a variant payload field is `bynk.types.field_default_on_variant`. See [Field defaults](/book/reference/types/#field-defaults) and [Add an option without breaking callers](/book/guides/program-structure/add-an-option/)."
---

## ADR: record-field-defaults
title: Record field defaults on every record type, applied at the literal and at the decode
summary: Optional inputs stay in options records; their fields take defaults, so function arity stays fixed

**Context.** Bynk has no optional parameters and no overloading (ADR 0051), so an
operation's optional inputs travel in an options record beside the plain
operation (`Fetch.sendWith`, `R2.putWith`, `Queue.sendWith`). A record literal
had to give every field (`bynk.resolve.missing_field`), so every options literal
spelt out every option, and adding a field to any record broke every literal of
it: ADR 0655 had to correct ADR 0022 ("widening `Request` is breaking") after
ADR 0453 migrated each `Request { … }` by hand. Field defaults already existed:
parsed on every record body, validated and decoded for `event` fields (#972) and
for `migrate` old-shape types (#2224), and rejected everywhere else with
`bynk.event.default_outside_event`. Proposed and agreed in #2607.

**Decision.**

- **A. Defaults on every record type's fields, not on variant payloads.** A
  variant is built like a call, so a payload default would be an optional
  argument by another name; the parser rejects one with
  `bynk.types.field_default_on_variant`. Function declarations, function types
  and call arity are unchanged.
  - **Rejected: parameter defaults and arity overloading.** They make a
    function's arity ambiguous as a value, raise the question of whether a
    capability interface or its provider owns a default, need named arguments
    to skip a middle parameter, and would put a second way to pass options
    beside the `…With` record. ADR 0174 already declined overload dispatch.
- **B. A default is the event rule's static, wire-representable value,** checked
  by the same `check_static_initialiser` and `lower_field_default_wire` path:
  ADR 0004's closed set, no `self`, parameter, binding, capability or function.
  The set gains `Map.empty()` and `Set.empty()`, both `[]` on the wire (a map
  travels as its entries), so the options records' `metadata` maps can be
  defaulted. A non-empty map default stays out: there is no map literal.
- **C. A default applies at the literal and at the decode, nowhere else.**
  - A literal that leaves a defaulted field out takes the default. The emitter
    writes it as a self-contained value (`lower_field_default_value`:
    `{ tag: "Follow" }`, `new globalThis.Map()`) rather than re-lowering the
    default's source, because the literal may build a record declared in
    another unit (`FetchOptions`), whose default names values (`Redirect.Follow`)
    the literal's module never imports.
  - A spread (`R { ...base, f: v }`) takes every unwritten field from `base`.
  - A decode of a body with no key for a defaulted field takes the default; a
    present key, `null` or an explicit `None` included, decodes as sent. Every
    boundary already shared the record codec that events used, so this needed
    no new decode path.
  - A record literal written as a default may leave out its own type's
    defaulted fields.
- **D. An `Option` field is required in a literal unless it says `= None`.** An
  implicit `None` would make whether a field is optional depend on its type
  rather than on its declaration, and give one meaning two spellings. Decoding
  is unchanged: an absent key at an `Option` field was already `None`.
- **E. Defaults are part of a type's source contract, not its wire hash.** Adding
  a field still changes a service's contract hash, defaulted or not, and changing
  only a default still does not. Adding a field with a default no longer breaks
  literals; adding one without a default still does. Changing a default changes
  every literal and decode that relies on it, so within the stable `bynk` unit it
  is a breaking change. The OpenAPI document lists a defaulted field, including
  one of a generic record's instantiation, as not required.
- **F. Stored agent state decodes the default; the schema lock is unchanged.**
  Rehydration uses the record codec, so a record in a `Cell[T]` that gains a
  defaulted field loads an instance stored before it with the default. Whether
  the agent schema lock should count such a nested field as additive, as the
  event registry does, is a follow-on: it changes `bynk.schema.lock` semantics.
- **G. Diagnostics.** `bynk.event.bad_field_default` becomes
  `bynk.types.bad_field_default`, now reported for record, event and old-state
  defaults alike. The proposal kept the old code as an alias for a release, but
  the registry admits only codes the compiler emits, so it is renamed outright
  and the rename is in the code's description. `bynk.event.default_outside_event`
  is retired, and `bynk.types.field_default_on_variant` is new.
- **H. The first-party options records default to the plain operation:**
  `FetchOptions` (`timeout = None`, `redirect = Redirect.Follow`), `QueueSend`
  (`delay = None`), `KvGet` (`cacheTtl = None`), `KvPut`
  (`expiration = KvExpiration.Never`, `metadata = Map.empty()`) and `R2Put`
  (`contentType = None`, `metadata = Map.empty()`, `onlyIf = R2Condition.Always`).
  An options record beside a plain operation gives every field the default that
  reproduces it, so `sendWith(req, FetchOptions {})` is `send(req)`.
- **I. A generic record's default holds for every instantiation.** A field whose
  type is a bare type parameter has none, nor does a default that reaches one
  (`Option[T] = Some(1)`); `Option[T] = None`, `List[T] = []`, `Map.empty()`,
  `Set.empty()` and a concrete field's default do. A field whose type applies a
  user generic (`Page[T]`, `Page[Int]`) has no default in this increment. The
  generic codec carries each default typed at the instantiation, so it decodes
  as a non-generic record's does.

**Consequences.** Existing programs compile unchanged, and emission is unchanged
for every literal that gives every field. An option is added to an operation by
adding a defaulted field to its options record, without touching a caller or an
old stored or sent value. The editor needs nothing new: hover already renders a
field default, the "add missing fields" quick fix offers only fields without one,
and signature help is untouched because no signature changed. Follow-ons:
agent-schema additivity for defaulted nested fields (F), a hash that ignores an
added defaulted field (E), and defaults on fields that apply a user generic (I).
