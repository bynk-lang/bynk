# Record field defaults on every record type

<!-- Draft (untracked) per design/proposals/README.md §Drafting. Promote to an
issue from the increment-proposal template, label `proposal`, then delete. -->

> **Draft status (31/10/2026).** Settled with the maintainer: A (record
> bodies only), D (no implicit `= None`), E (contract hash rule unchanged) and
> F (agent-storage additivity deferred). B, C, G and H stand as recommended,
> pending review.

## Summary

- **Scope:** checker (`bynk-check`) and emitter (`bynk-emit`); the book, the
  spec and the diagnostics reference; editor tooling for completion and quick
  fixes. The grammar and AST are untouched: `RecordField.init` is already
  parsed on every record body. Function declarations, function types, call
  arity and sum-variant payloads are untouched too.
- **Addresses:** two linked gaps. First, Bynk has no optional parameters and no
  overloading (ADR 0051), so optional inputs travel in an options record beside
  the plain operation (`Fetch.sendWith`, `R2.putWith`, `Queue.sendWith`). But a
  record literal must give every field (`bynk.resolve.missing_field`,
  `bynk-check/src/resolver.rs:2421`), so every options literal spells out every
  option. Second, adding a field to any record breaks every literal of it. ADR
  0655 had to correct ADR 0022 over this ("widening `Request` is breaking"),
  after ADR 0453 had migrated every `Request { … }` literal by hand.
- **Realises:** any record type's field may declare `field: T = expr`. A record
  literal may leave out a defaulted field, which then takes its default, and a
  boundary decode of a body with no key for it does the same. Adding a field
  with a default is no longer a breaking change at the source level.

## Framing (why this is the language's to fix)

The language already has the mechanism. Field defaults are parsed on every
record, validated as static wire-representable values, lowered into
deserialisers, and rendered on hover. They are switched on for `event` fields
(events slice 3a, #972) and for `migrate` old-shape types, and are rejected
everywhere else. The rejection's own note says why: "an ordinary record has no
wire-evolution story". That note no longer holds. Ordinary records now cross
HTTP and queue boundaries, ADR 0655 records a source-evolution story for them,
and the options-record convention depends on records being cheap to build.

The alternatives, parameter defaults and overloading, are worse for this
language:

- **Function types.** They would make a function's arity ambiguous when it is
  passed as a value (`bynk.types.lambda_mismatch`).
- **Capabilities.** They raise the question of whether an interface or its
  provider owns a default. Swift bans defaults in protocols for this reason.
- **Named arguments.** Skipping a middle parameter needs them, a second feature.
- **Principles.** They would break "one canonical way", since defaults would
  sit beside `…With` records, and "cost visible at the call site". ADR 0174 has
  already declined overload dispatch.

This proposal leaves function arity fixed and makes the existing convention
pleasant to use.

## What exists today (grounded)

- **Parsing.** `RecordField.init` is parsed for every record body (v0.11
  grammar); see the comment at `bynk-check/src/checker/refinements.rs:50-60`.
- **Rejection.** `check_type_decl` rejects `init` unless `allows_field_defaults`
  (`refinements.rs:61`), with `bynk.event.default_outside_event`.
- **Validation.** Event defaults go through `check_static_initialiser` and
  `wire_default::lower_field_default_wire` (`bynk-check/src/wire_default.rs`).
  A default must be static and wire-representable: a literal, a sum variant,
  `Some`/`None`/`Ok`/`Err`, a record, or `T.unsafe(lit)`. It may not use
  `self`, a parameter or a capability (ADR 0004's set). A failure is
  `bynk.event.bad_field_default`.
- **Decoding.** A defaulted field whose key is absent decodes to the default.
  A key that is present but `null`, or `{ "kind": "None" }`, is a real `None`,
  not the default. An `Option` field with no default already decodes an absent
  key as `None` (understand-events.md, "Evolving an event's shape"). The
  agents reference already warns that a `migrate` type's defaults apply at any
  boundary it is used at, so the deserialisers already honour `init` beyond
  events.
- **Literals.** Literal checking ignores `init`. Even an event literal must
  give every field. The quick fix's `field_default_init` (`resolver.rs:2457`)
  offers type zeros (`0`, `""`, `None`, `[]`), not declared defaults.
- **Hover.** Hover already renders a field default (`bynk-ide/src/symbols.rs:1805`).
- **Contract hash.** The service contract hash does not cover field defaults
  (wire-format.md, "Changes the hash" table).
- **No record patterns.** A `_pattern` is a wildcard, a literal, a binding, a
  variant or an or-pattern (syntactic-grammar §4.7.2). Event delivery filters
  list a subset of fields. So no exhaustive record pattern can break when a
  field is added.
- **No schema generation.** Nothing in the compiler generates an
  OpenAPI/JSON Schema document, so no external schema has to carry `default`.

## The surface

```bynk
type FetchOptions = {
  timeout: Option[Duration] = None,
  redirect: Redirect = Redirect.Follow,
}

Fetch.sendWith(req, FetchOptions { timeout: Some(5.seconds) })
Fetch.sendWith(req, FetchOptions {})       // every field defaulted
```

A field with a default may be left out of a record literal. A field without
one may not, exactly as today.

## Decisions

**[DECISION A] Where defaults are allowed (Recommended: every `type X = { … }`
record body; not sum-variant payloads).** This lifts `allows_field_defaults`
to true for record type declarations. Variant payloads such as
`Variant(field: T)` are left alone. They are constructed positionally or by
name like a call, and defaults there would bring back exactly the
optional-argument problem this proposal avoids. The `migrate` guidance in the
agents reference ("keep old types dedicated") becomes redundant and is reworded.

**[DECISION B] What a default may be (Recommended: unchanged, the event
rule).** The same static, wire-representable set, validated by the same
`check_static_initialiser` and `lower_field_default_wire` path: ADR 0004's
closed set, pure, no `self`, parameters, capabilities or free functions. A
non-wire type, such as a function-typed field or a held resource, cannot
declare a default (`bynk.types.bad_field_default`, below). One rule for every
record keeps "one canonical way" and reuses the code that exists.

The set gains one admission: `Map.empty()` and `Set.empty()`. Their wire forms
are `{}` and `[]`. Today `lower_field_default_wire` rejects every `Map`/`Set`
default ("no closed-form default literal", `wire_default.rs:100-101`). Without
this, `KvPut.metadata` and `R2Put.metadata` could not be defaulted (Decision
H). Both are nullary, pure and closed-form, which is what ADR 0004 asks of a
static expression. A non-empty map default stays out, because the language has
no map literal.

**[DECISION C] Where a default is applied (Recommended: at the construction
site and at the decoding boundary, nowhere else).**

- **Literal.** A literal that leaves out a defaulted field lowers as though the
  field were written with its default expression. This is well-defined because
  the expression is static and has no free names: the value is the same in
  every scope.
- **Spread.** A spread `X { ...base, f: v }` takes every unwritten field from
  `base`, never from the default. The default fills only a field that is
  neither written nor supplied by a spread.
- **Decode.** A boundary decode (HTTP body, queue message, event, rehydration)
  of a body with no key for a defaulted field takes the default. A key that is
  present but `null`, or an explicit `None`, keeps its explicit value, as
  events already do.
- **Elsewhere.** Nothing else changes: field access, equality and
  serialisation always see a complete value.

**[DECISION D] `Option` fields (Recommended: no implicit `= None` in
literals).** An `Option` field with no written default is still required in a
literal. Wire decoding is unchanged: an absent key already decodes to `None`,
with or without a default. Making `None` implicit would give two spellings of
the same type, and would make "is this optional?" depend on the type rather
than on what the declaration says. The cost is writing `= None` once per field
in the declaration, not once per literal.

**[DECISION E] Wire contract and stability (Recommended: defaults are part of
a type's source contract, not its wire hash, and the hash rule stays as it
is).**

- **Hash.** Adding a field still changes a service's contract hash, as the
  wire-format table says, defaulted or not. Changing only a default's value
  still does not. Letting an additive defaulted field keep the hash is a
  separate wire-version decision and is left out of scope.
- **Literals.** Adding a field with a default no longer breaks literals.
  Adding one without a default still does.
- **Changing a default** is a behaviour change for every literal that leaves
  the field out, and for every old body decoded without the key. The book says
  so. Within the stable `bynk` unit, changing a default's value is treated as a
  breaking change under the release discipline.
- **Removing a default** breaks every literal that omitted the field. The
  compiler already flags it at each literal, through `missing_field`.

**[DECISION F] Stored agent state (Recommended: decode honours the default,
but the agent schema registry's additive rule is unchanged in this
increment).** Rehydration uses the boundary deserialiser, so a record in a
`Cell[T]` gains a defaulted field cleanly on load, which is strictly better
than today's structural fault. However, the agent schema lock still treats a
change to a nested record's structure as needing a `migrate` step. Whether a
defaulted nested field counts as additive for agents, as it already does for
events, is a named follow-on. It touches `bynk.schema.lock` semantics, which
deserve their own slice.

**[DECISION G] Diagnostics (Recommended: retire one code, rename one).**
`bynk.event.default_outside_event` is retired; it is only still reported on a
variant payload, as `bynk.types.field_default_on_variant`.
`bynk.event.bad_field_default` becomes `bynk.types.bad_field_default`, and the
old code is kept as an alias in the diagnostics reference for one release. The
quick fix for `missing_field` is unchanged except that it can no longer fire
for a defaulted field.

**[DECISION H] Standard library (Recommended: add defaults to the existing
options records in the same increment).** Each default reproduces the plain operation:

- `FetchOptions`: `timeout = None`, `redirect = Redirect.Follow` (`bynk-std/lib/core/bynk.bynk:74`).
- `QueueSend`: `delay = None`.
- `KvPut`: `expiration = KvExpiration.Never`, `metadata = Map.empty()`.
- `R2Put`: `contentType = None`, `metadata = Map.empty()`, `onlyIf = R2Condition.Always`.

These are in `bynk-std/lib/cloudflare/bynk.cloudflare.bynk`. This is non-breaking: existing literals still
compile. It is the increment's motivating proof. It also makes the convention
in ADR 0655 binding: an options record's fields carry defaults that reproduce
the plain operation, so `sendWith(req, FetchOptions {})` is exactly `send(req)`.

## The deltas (concretely)

- **Grammar / AST (`bynk-syntax`).** None, because `RecordField.init` already
  parses on every record body. Add the new and renamed diagnostic codes to
  `diagnostics.rs`.
- **Checker (`bynk-check`).**
  - Allow `init` on record type declarations (`refinements.rs`).
  - Run the event-default validation for every record field with `init`.
  - Make the `missing_field` check skip a field that has `init`
    (`resolver.rs:2405`), and the matching check in `project_model.rs:5673`.
  - Report `field_default_on_variant` for a default on a variant payload.
- **Emitter (`bynk-emit`).**
  - **Literals:** lower a literal with omitted defaulted fields by inserting
    each default's value.
  - **Deserialisers:** extend the absent-key-to-default path from event and
    migrate types to every record type.
- **Runtime.** None. Defaults are inlined at emission.
- **Standard library.** Defaults on the options records (Decision H).

## Risks & mitigations

- **A default changes silently under consumers.** Mitigation: Decision E
  treats it as breaking within the stable unit, and the book documents it.
  Hover already shows the default at every use.
- **Defaults hide meaningful choices** (for example, a record that should
  force the author to pick a policy). Mitigation: defaults are opt-in per
  field. The guide says to leave a field undefaulted when there is no safe
  neutral choice.
- **A default may diverge from what a type is used for.** For instance, a
  default suited to a request body is wrong for stored state. Mitigation:
  there is one rule, stated plainly. A default applies everywhere the type is
  built or decoded. The `migrate` guidance generalises to "a type's defaults
  go wherever the type goes".
- **Fixtures and goldens churn.** Mitigation: none expected, because emission
  for literals that give every field is unchanged.

## Docs delta

- **Spec.** In static-semantics, update the record-literal well-formedness
  section and the event field-default section (now a special case of the
  general rule), and the `migrate` old-shape note in §5.4. Add the defaulting
  rule to the boundary decoding section in emission.
- **Reference.** Records/types reference: field defaults, the spread rule and
  the `Option` rule. Diagnostics reference: retire, rename and add codes.
  Wire-format: a note that defaults apply on decode. Agents: reword "keep old
  types dedicated".
- **Guide.** A "Do" recipe, "Add an option without breaking callers", showing
  the `…With` plus options-record convention with defaults. Edit
  understand-events.md "Evolving an event's shape" so it no longer says a
  plain record's default is rejected. Not a new concept, so no new "Understand"
  page.
- **Changelog + version history.** An entry at the version this ships as, and
  the currency banner and appendix-version-history advanced to it.
- **Roadmap.** Name agent-storage additivity (Decision F) and
  hash-preserving additive fields (Decision E) as follow-ons.

## Tooling delta (ADR 0156 — silence is an oversight)

- **Hover:** unchanged, because it already renders a field default
  (`bynk-ide/src/symbols.rs:1805`), now for every record.
- **Completion:** changed. Inside a record literal, field completion lists
  undefaulted fields first and marks defaulted ones with their default. The
  "add all missing fields" quick fix inserts only undefaulted fields.
- **Semantic tokens:** unchanged, because a default is an ordinary expression
  in an already-tokenised position.
- **Signature help:** unchanged, because function signatures and arity do not
  change. Hover on an options-record parameter shows the record's defaults.

## Done when

- `type R = { a: Int, b: Int = 3 }` compiles, `R { a: 1 }` evaluates to
  `{ a: 1, b: 3 }`, and `R { a: 1, b: 2 }` is unchanged.
- `R {}` is `missing_field` for `a` only.
- `R { ...r0, a: 9 }` takes `b` from `r0`, not from the default.
- An HTTP body `{"a":1}` decoded as `R` yields `b = 3`.
- An `Option` field with an explicit default and a body key `null` decodes to
  `None`.
- A non-static default is `bynk.types.bad_field_default`, and a variant-payload
  default is `bynk.types.field_default_on_variant`.
- `Fetch.sendWith(req, FetchOptions {})` behaves exactly as `send(req)`.
- Fixtures (next free indices) cover each case above. The literal and decode
  semantics are proved by a behavioural fixture (`expected_run.txt`, run by
  `bynkc/tests/behaviour_fixtures.rs`), not only by goldens.
- Docs are current per the delta above, and the tooling surfaces are as stated.
- A version bump (`scripts/bump-version.sh`).
- A new ADR records Decisions A to H. Its number is assigned when the
  implementing PR lands, and the PR closes the proposal issue.
