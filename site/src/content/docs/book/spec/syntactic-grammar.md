---
title: "§4 Syntactic grammar"
---
This chapter defines Bynk's phrase structure: how tokens ([§3](/book/spec/lexical-grammar/))
combine into declarations, types, expressions, patterns, and statements. Each
production is generated from the grammar ([§2.1](/book/spec/conventions/)) and embedded by
name.

A production states what **parses**. Every constraint beyond parsing — name
resolution, typing, exhaustiveness, refinement admission, the effect discipline,
and all other well-formedness — is a static-semantics rule, specified normatively
in **§5** and not repeated here. Where a construct carries such constraints, this
chapter forward-references §5 rather than restating them.

The chapters mirror the construct groupings of the friendly
[grammar reference](/book/reference/grammar/); the productions are shared, the
register here is the normative definition.

## §4.1 Top-level & modules

A source file is a `commons`, a `context`, an `adapter`, or test declarations.

### §4.1.1 source_file

{{#grammar source_file}}

A whole file: one or more top-level declarations, or a single fragment used by
editor tooling.

### §4.1.2 item_fragment

{{#grammar _item_fragment}}

A tooling entry point: a single body item parsed in isolation. Not written by
hand.

### §4.1.3 expr_fragment

{{#grammar _expr_fragment}}

A tooling entry point: statements and/or an expression parsed in isolation. Not
written by hand.

### §4.1.4 commons_decl

{{#grammar commons_decl}}

A `commons` module. The body braces are optional at file scope; with no braces
the body items run to the end of the file.

### §4.1.5 context_decl

{{#grammar context_decl}}

A `context`. As with `commons`, the body braces are optional at file scope.
Well-formedness: §5.

### §4.1.6 adapter_decl

{{#grammar adapter_decl}}

An `adapter` — the host boundary: a capability contract co-located with a named
TypeScript binding. As with `commons`, the body braces are optional at file
scope. An adapter's providers are **external** (bodiless,
[§4.3.8](#438-provider_decl)) and it may not declare services or agents; those
placement rules, the binding requirement, and the reserved `bynk` namespace are
well-formedness: §5.

### §4.1.7 suite_decl {#416-test_decl}

{{#grammar suite_decl}}

A `suite` block naming the `commons` or `context` it targets. A suite header may
carry an optional `as <tier>` clause (§4.9) that sets the default tier its cases
inherit. Well-formedness: §5.

### §4.1.8 commons_body_item

{{#grammar _commons_body_item}}

The declaration forms admitted in a `commons` body.

### §4.1.9 context_body_item

{{#grammar _context_body_item}}

The declaration forms admitted in a `context` body, including `consumes` and
`exports`.

### §4.1.10 adapter_body_item

{{#grammar _adapter_body_item}}

The declaration forms admitted in an `adapter` body: the `binding` clause,
capability and type declarations, pure helpers and `uses`, `consumes`,
`exports`, and providers. The grammar is deliberately permissive — `service` and
`agent` parse here so the placement error can be precise; their rejection is
well-formedness: §5.

### §4.1.11 test_body_item

{{#grammar _test_body_item}}

The declaration forms admitted in a `suite` body: `uses`, `stub` and `env` clauses, and
`case` / `property` declarations.

### §4.1.12 event_decl (v0.238) {#4112-event_decl}

{{#grammar event_decl}}

`event`, a name, zero or more `@name(args)` annotations, `=`, and a record body:
a typed fact a context emits with the `Events` capability and other contexts
receive with a `from Events(E)` subscription ([§4.4.7b](#447b-events-subscriptions-v0238)).
The body is a record type only; its fields may carry a default (`field: T =
expr`). The grammar admits an `event` in any body, and the annotation list in
any number. Placement (contexts only), the closed annotation set (`@schema(N)`,
at most once), field defaults, and emission are well-formedness: §5.7b.

### §4.1.12a asset_decl {#4112a-asset_decl}

{{#grammar asset_decl}}

`asset`, a name, `=`, a plain string literal naming a file relative to the
project root (an interpolated string is a parse error here), and optionally
`at` and a second plain string literal: a fixed URL path to serve the file at.
`at` is contextual, recognised only in this position. The grammar admits
an `asset` in any body. Placement (contexts and commons only), the path rules,
and the declared name's type are well-formedness: §5.12.

### §4.1.12b embed_decl {#4112b-embed_decl}

{{#grammar embed_decl}}

`embed`, a name, `=`, and a plain string literal naming a file relative to the
project root (an interpolated string is a parse error here). The grammar admits
an `embed` in any body. Placement (contexts and commons only), the path rules,
and the declared name's type are well-formedness: §5.12a.

### §4.1.12c const_decl {#4112c-const_decl}

{{#grammar const_decl}}

`const`, a name, `:`, a type, `=`, and an expression: a named constant. The
type annotation is required; a declaration without one is a parse error
(`bynk.parse.const_missing_type`). The grammar admits a `const` in any body and
any expression as its initialiser. Placement (contexts and commons only), the
name's case, the closed set of initialisers and the cycle rule are
well-formedness: §5.12b. `const` is a contextual keyword, so it remains usable
as an ordinary identifier elsewhere.

### §4.1.12d visibility_modifier {#4112d-visibility_modifier}

{{#grammar visibility_modifier}}

`private`, written immediately before a `type`, `fn` (a free function or a
method), `messages`, `asset`, `embed` or `const` declaration, makes that
declaration unit-private. `private` is a contextual keyword (§3): it is the modifier only
in that position and an ordinary identifier everywhere else, so
`@cache(scope: private)` and a binding named `private` are unaffected. The
compiler also recognises `private` before a `service`, `agent`, `capability`,
`provides`, `actor` or `event` declaration, and in an adapter, in order to
reject it precisely (`bynk.visibility.private_not_allowed`). Visibility is
well-formedness: §5.1.

### §4.1.13 messages_decl (v0.228) {#4113-messages_decl}

{{#grammar messages_decl}}

A message bundle's block for one locale: `messages`, the locale tag as a string
literal, zero or more annotations (`@reference` marks the reference locale), and
a brace-delimited list of entries. The grammar admits it in any body;
placement (commons only), the tag's `LocaleTag` check, and the one-`@reference`
rule are well-formedness: §5.11.

### §4.1.14 message_entry {#4114-message_entry}

{{#grammar message_entry}}

One `"code" => "template"` entry in a `messages` block. Both sides are plain
string literals: an interpolated string (`"…\(x)…"`) is a parse error here. A
template's `{name}` placeholders are ICU MessageFormat, not Bynk syntax; they
are checked by §5.11, not parsed by the grammar. A doc on the lines above an
entry documents it (#1888).

### §4.1.15 qualified_name

{{#grammar qualified_name}}

A dotted sequence of identifiers, e.g. `shop.orders`. A dotted name is a single
**flat** identifier, not a hierarchy: `bynk` and `bynk.time` are independent
names that merely share a leading segment.

### §4.1.16 uses_decl

{{#grammar uses_decl}}

`uses` followed by a qualified name. Well-formedness: §5.

### §4.1.17 consumes_decl

{{#grammar consumes_decl}}

`consumes` a unit, in one of three forms: the **whole unit** (`consumes b`), the
whole unit under an **alias** (`consumes b as Alias`), or a **capability
selection** (`consumes b { Cap, … }`), which flattens the named capabilities into
the consumer's local capability namespace under their bare names. The target may
be a context or an adapter; which forms each consumer kind admits, and the
flattening and clash rules, are well-formedness: §5.

### §4.1.18 exports_decl

{{#grammar exports_decl}}

`exports`, one of `opaque` / `transparent` / `capability`, and a brace-delimited
identifier list. Well-formedness: §5.

### §4.1.19 binding_decl

{{#grammar binding_decl}}

An adapter's `binding` clause: the TypeScript module supplying its external
provider classes, as a string-literal path resolved relative to the adapter's
source file, with an optional `requires { … }` map of npm dependencies.
Well-formedness: §5.

### §4.1.20 binding_requirement

{{#grammar binding_requirement}}

One `"package": "range"` entry in a binding's `requires` map. Ranges MUST be
pinned; well-formedness: §5.

## §4.2 Types & refinements

Type declarations and the type references that appear in signatures.

### §4.2.1 type_decl

{{#grammar type_decl}}

`type`, a name, an optional `[A, B]` **type-parameter list** (v0.157 — only a
record or sum body may be generic; a parameter is a name, optionally bounded
(§4.3.1b), scoped to the declaration), `=`, a type body, and an optional
trailing **`derives` clause** (constrained generics, slice 1): `derives Ord`, or
`derives Ord + C` for a list. `derives` is a contextual keyword, recognised only
after a type body. The grammar admits any constraint name; the checker admits
only `Ord`. `Eq`, `Key` and `Codable` are derived automatically, so naming one is
`bynk.types.derives_automatic` (its fix removes it), and any other name is
`bynk.generics.unknown_constraint`. Every field and payload type of a type that
derives `Ord` MUST be `Ord` (`bynk.types.derives_field_not_orderable`).
Well-formedness: §5; the type system: §6.

### §4.2.2 type_body

{{#grammar _type_body}}

The right-hand side of a `type`: one of the five type forms.

### §4.2.3 opaque_type

{{#grammar opaque_type}}

`opaque`, a base type, and an optional `where` refinement.

### §4.2.4 refined_type

{{#grammar refined_type}}

A base type with an optional `where` refinement. Well-formedness: §5;
admission: §6.

### §4.2.5 record_type

{{#grammar record_type}}

A brace-delimited, comma-separated list of record fields, with an optional
trailing comma.

### §4.2.6 record_field

{{#grammar record_field}}

A field name, `:`, a type, an optional inline `where` refinement, and an optional
`=` default expression. Well-formedness: §5. A doc (a doc-block or a run of doc
lines, [§3.3](/book/spec/lexical-grammar/#33-comments-and-doc-blocks)) on the
lines above a field documents it (#1888).

### §4.2.7 sum_type

{{#grammar sum_type}}

One or more `|`-prefixed variants, followed by an optional trailing
`embeds <type> as <Variant>, …` clause (v0.154) declaring **error embeddings**
— mappings a value of `<type>` auto-wraps into the named single-payload
variant, which the `?` operator uses for automatic error conversion
([§5.2](/book/spec/static-semantics/)). `embeds` and `as` are contextual
keywords here. Well-formedness: §5.

### §4.2.8 sum_variant

{{#grammar sum_variant}}

A `|`, a constant name, and an optional parenthesised payload. A doc on the lines
above the `|` documents the variant (#1888); the first variant's doc follows the
`=`.

### §4.2.9 variant_payload_field

{{#grammar variant_payload_field}}

A named field in a sum-variant payload: an identifier, `:`, and a type. A doc on
the lines above it, inside the parentheses, documents it (#1888).

### §4.2.10 enum_type

{{#grammar enum_type}}

`enum` and a brace-delimited list of constant names — a sum type whose variants
all carry no payload. A doc on the lines above a name documents that variant
(#1888).

```bynk
type Shape =
--- A circle.
| Circle(
	--- The radius, in pixels.
	radius: Int,
)
--- A single point.
| Dot

type Outcome = enum {
	--- Every check passed.
	Pass,
	Fail,
}
```

### §4.2.11 refinement

{{#grammar refinement}}

One or more predicates joined by `&&`. Well-formedness: §5.

### §4.2.12 refinement_pred

{{#grammar _refinement_pred}}

A single predicate: a predicate call or a bare predicate name.

### §4.2.13 pred_call

{{#grammar pred_call}}

A predicate name applied to parenthesised arguments, e.g. `InRange(1, 100)`.

### §4.2.14 predicate_name

{{#grammar predicate_name}}

The set of built-in refinement predicates. Well-formedness: §5.

### §4.2.15 pred_arg

{{#grammar _pred_arg}}

An argument to a predicate: a number or string literal.

### §4.2.16 base_type

{{#grammar base_type}}

The primitive types `Int`, `String`, and `Bool`. Well-formedness: §5.

### §4.2.17 type_ref

{{#grammar _type_ref}}

A type as it appears in a signature: a function type, a base type, the unit
type, the validation-error type, a generic application, or a named type.

### §4.2.17a function_type_ref

{{#grammar function_type_ref}}

A function type (v0.20a): `Int -> Int`, `(Int, String) -> Bool`, `() -> Int`.
The arrow is **right-associative** — `A -> B -> C` is `A -> (B -> C)` — and a
parenthesised list before `->` is a parameter list (a single parenthesised
type without an arrow is a grouping; the empty `()` without an arrow stays
the unit type). A function type is **effectful** exactly when its return type
is `Effect[_]` — the structural rule of §6. Function types are confined to
non-boundary positions; well-formedness: §5.

### §4.2.18 unit_type

{{#grammar unit_type}}

The unit type `()`.

### §4.2.19 validation_error_type

{{#grammar validation_error_type}}

`ValidationError`, the error type produced when refined-type validation fails.

### §4.2.20 generic_type_ref

{{#grammar generic_type_ref}}

A generic constructor — `Result`, `Option`, `Effect`, `HttpResult`, or
(v0.20b) `List`, `Map` — applied to bracketed type arguments.
Well-formedness: §5 (`Map` keys are value-keyable,
[§5.10](/book/spec/static-semantics/#510-collections)); the type system: §6.

### §4.2.20a applied_type_ref

{{#grammar applied_type_ref}}

An application of a **user-declared generic type** (v0.157) to bracketed type
arguments — `Paginated[User]`, `Keyed[String, Int]`. The head is a user type
name declared `type Name[T, …] = { … }` (a record). A generic type must be
applied to exactly its declared number of arguments, and a generic record is a
**non-boundary** value. Well-formedness: §5; the type system: §6.

## §4.3 Functions, capabilities & providers

Pure functions and methods, capability interfaces, and the providers that
implement them.

### §4.3.1 fn_decl

{{#grammar fn_decl}}

`fn`, a function name or a `Type.method` name, an optional `[A, B]`
**type-parameter list** (v0.20a; each parameter may carry bounds, §4.3.1b), a
parameter list, `->`, a return type, an optional **`where` clause** (§4.3.1b), any
number of **contract clauses** (v0.115), and a block body. Well-formedness: §5.

### §4.3.1b type_param / where_clause (constrained generics, slice 0)

{{#grammar type_param}}

A type parameter: a name, optionally followed by `:` and one or more
**constraint** names joined by `+` (`T`, `T: Eq`, `T: Eq + C`). The same form
appears on `fn`, `type` and capability-operation type-parameter lists. The
grammar admits any constraint name; the checker admits `Eq` and `Ord`
(`bynk.generics.unknown_constraint`), and rejects a bound on a capability
operation (`bynk.generics.bound_on_capability_op`). Well-formedness: §5.2.

{{#grammar where_clause}}

{{#grammar where_bound}}

A method's **`where` clause** follows the return type and precedes any contract
clause. Each entry bounds a type parameter of the method or of its receiver
type, which the method does not declare itself: `fn Box.has(self, x: A) -> Bool
where A: Eq`. An entry naming anything else is
`bynk.generics.unknown_where_param`.

### §4.3.1a requires_clause / ensures_clause (v0.115)

{{#grammar requires_clause}}

{{#grammar ensures_clause}}

A function **contract** (testing track slice 3): named `requires` (preconditions)
and `ensures` (postconditions) between the return type and the body, each a pure
`Bool` predicate expression — the one predicate surface (§4.6, ADR 0144). A
`requires` predicate is over the parameters; an `ensures` predicate is over the
parameters and `result`, the contextual binding for the return value (an ordinary
identifier elsewhere). Well-formedness — purity, `Bool` type, `result` scope,
distinct names: §5 (ADR 0150).

### §4.3.2 method_name

{{#grammar method_name}}

A `Type.method` name, defining a method on a named type.

### §4.3.3 params

{{#grammar _params}}

A parameter list: an optional `self` receiver followed by named parameters, with
an optional trailing comma.

### §4.3.4 self_param

{{#grammar self_param}}

The `self` receiver of a method or handler.

### §4.3.5 param

{{#grammar param}}

One parameter: an identifier, `:`, and a type. Well-formedness: §5. A function,
method or handler parameter is always a name: destructuring a record in a
signature is a non-goal of the patterns track (it would put patterns into
signatures, hover, signature help and the cross-context contract). Only a lambda
parameter may be a record pattern (§4.6.9b); to destructure a `fn` parameter,
write a `let` pattern as the first statement of the body.

### §4.3.6 capability_decl

{{#grammar capability_decl}}

`capability`, a name, then either a brace-delimited list of operation
signatures, or `=`, a base capability and zero or more `@annotation` options: a
**derived** capability (`capability Sessions = Kv`, standard-library S4a), a
distinct instance of the base. Well-formedness: §5.5.

### §4.3.7 capability_op

{{#grammar capability_op}}

One operation in a capability: `fn`, a name, an optional `[T, …]` type-parameter
list (v0.235, ADR 0281), parameters, `->`, and a return type — no body. A type
parameter is resolved only from an explicit type argument at the call site
(`Cap.op[SomeType](…)`), never inferred, and carries no bound
(`bynk.generics.bound_on_capability_op`). Well-formedness: §5.

### §4.3.8 provider_decl

{{#grammar provider_decl}}

`provides`, the capability name, `=`, an implementation name, an optional `given`
clause, and an **optional** brace-delimited list of operation implementations.
The presence of the brace block distinguishes the two provider kinds: with a
block the provider is implemented **in Bynk** (context-only); with no block it is
**external** — its implementation is the named class exported by the enclosing
adapter's binding module ([§4.1.19](#4119-binding_decl)). The absence of the
block, not an empty one, is the signal. Placement and wiring rules:
well-formedness, §5.

### §4.3.9 provider_op

{{#grammar provider_op}}

One operation implementation: a capability operation signature with a block body.
Well-formedness: §5.

### §4.3.10 given_clause

{{#grammar given_clause}}

`given` and a comma-separated list of the capabilities a handler or provider may
use. Well-formedness: §5.

## §4.4 Services & handlers

A `service` groups the handlers that respond to calls and external triggers.

### §4.4.1 service_decl

{{#grammar service_decl}}

`service`, a name, an optional `from <protocol>` header clause, and a
brace-delimited list of handlers. One protocol per service. Well-formedness: §5.

### §4.4.1a HTTP service policies (v0.131, v0.141, v0.142)

A `from http` service may open with up to three policy sections, in header
position before its handlers and in this order: `cors { }`, then `security { }`,
then `limits { }`, each optional (the order is `service_decl`'s). A policy
out of that order, or after a handler, is a parse error
(`bynk.parse.policy_order`). Each is a
brace-delimited list of `name: value` fields. `cors`, `security` and
`limits` are contextual keywords, ordinary identifiers elsewhere. The grammar
admits any field name and any value expression; the closed field sets, their
value types, and the `from http`-only placement are well-formedness:
[§5.7.1](/book/spec/static-semantics/#cors),
[§5.7.3](/book/spec/static-semantics/#security) and
[§5.7.4](/book/spec/static-semantics/#body-limits).

#### cors_policy {#441a1-cors_policy}

{{#grammar cors_policy}}

The cross-origin policy (v0.131).

#### cors_field {#441a2-cors_field}

{{#grammar cors_field}}

One `name: value` field of a `cors` policy.

#### security_policy {#441a3-security_policy}

{{#grammar security_policy}}

The security-headers policy (v0.141).

#### security_field {#441a4-security_field}

{{#grammar security_field}}

One `name: value` field of a `security` policy.

#### limits_policy {#441a5-limits_policy}

{{#grammar limits_policy}}

The request-limits policy (v0.142).

#### limits_field {#441a6-limits_field}

{{#grammar limits_field}}

One `name: value` field of a `limits` policy.

### §4.4.2 service_protocol

{{#grammar service_protocol}}

The `from <protocol>` clause: `from http`, `from cron`, `from queue("name")`
(v0.44), `from websocket(in: I, out: O)` (v0.103), or `from Events(E)` (v0.238),
whose event may carry a delivery pattern and a `via schema(N)` clause
([§4.4.7b](#447b-events-subscriptions-v0238)). Absent ⇒ the contract-mediated
default, which admits only `on call`. Well-formedness: §5.

### §4.4.2a handler

{{#grammar handler}}

A handler: a call, HTTP, cron, queue, WebSocket, or event entry point, matching
the service's protocol. Well-formedness: §5.

### §4.4.3 call_handler

{{#grammar call_handler}}

`on call`, an optional name, parameters, `->`, a return type, an optional `given`
clause, and a block body.

### §4.4.4 http_handler

{{#grammar http_handler}}

`on <Method>("route")` — an HTTP method-builder (the verb collapses verb+route
into one config expression in the handler-config slot), then parameters, `->`, a
return type, an optional `given` clause, and a block body. Valid only in a
`from http` service. Well-formedness: §5.

### §4.4.4a rejected_handler

{{#grammar rejected_handler}}

`on rejected`, parameters, `->`, a return type, optional `by` and `given` clauses,
and a block body (#2341). Valid only in a `from http` service, at most once; it
renders the router's own rejections. That it takes exactly one
`BoundaryRejection`, returns `HttpResult[()]` and has no `by` or `given` is
well-formedness: §5.

### §4.4.5 http_method

{{#grammar http_method}}

The HTTP verbs a route may handle. Well-formedness: §5.

### §4.4.6 cron_handler

{{#grammar cron_handler}}

`on schedule("expr")`, parameters, `->`, a return type, an optional `given`
clause, and a block body. Valid only in a `from cron` service. Well-formedness: §5.

### §4.4.7 queue_handler

{{#grammar queue_handler}}

`on message(message)` — the bound queue lives on the service's `from
queue("name")` header. Parameters, `->` `Effect[QueueResult]`, an optional
`given` clause, and a block body. Well-formedness: §5.

### §4.4.7a WebSocket handlers (v0.103)

A `from websocket(in: I, out: O)` service declares the connection-lifecycle
handlers `on open`, `on message`, and `on close`. Each is a handler head — `on`,
the lifecycle keyword, parameters, `->` `Effect[()]`, a required
[`by_clause`](#448-by_clause) naming the actor, an optional `given` clause, and a
block body — and is valid only in a `from websocket` service:

- **`on open`** — the upgrade handshake; the body sees an owned `connection`
  binding of type `Connection[O]`.
- **`on message`** — parameters end with the decoded inbound frame of type `I`.
- **`on close`** — the connection ended.

`on message` reuses the [`queue_handler`](#447-queue_handler) production. The
grammar admits the `by` clause on every head as optional; that `on open` names
its actor is well-formedness. Well-formedness — exactly one `on open`, edge
authentication, held-resource disposal: §5.

#### ws_open_handler {#447a1-ws_open_handler}

{{#grammar ws_open_handler}}

`on open`, parameters, `->`, a return type, optional `by` and `given` clauses,
and a block body.

#### ws_close_handler {#447a2-ws_close_handler}

{{#grammar ws_close_handler}}

`on close`, with the same shape as `on open` (v0.106).

### §4.4.7b Events subscriptions (v0.238) {#447b-events-subscriptions-v0238}

A `from Events(E)` service subscribes to the event `E`
([§4.1.12](#4112-event_decl)) and has one handler, `on event`. Its header may
narrow delivery with a payload pattern, `from Events(E { field: value, .. })`
(v0.239), and with a version clause after the closing `)`, `via schema(N)`
(v0.244); a header carries either, both, or neither. `via` is part of the
`Events` protocol's production, so `via` on any other protocol is a syntax
error. Well-formedness: §5.7b.

#### event_handler {#447b1-event_handler}

{{#grammar event_handler}}

`on event`, parameters (the payload, then optionally an `EventEnvelope`), `->`, a
return type, optional `by` and `given` clauses, and a block body.

#### event_pattern {#447b2-event_pattern}

{{#grammar event_pattern}}

A brace-delimited list of `name: value` fields closed by a required `..`. The
pattern lists a subset of the event's fields; `{ }` with no fields is a parse
error (`bynk.parse.event_pattern_empty`), and the unfiltered form is the bare
`from Events(E)`.

#### event_pattern_field {#447b3-event_pattern_field}

{{#grammar event_pattern_field}}

One `name: value` entry in an `event_pattern`.

#### event_pattern_value {#447b4-event_pattern_value}

{{#grammar event_pattern_value}}

The value a field is matched against: an `Int`, `String` or `Bool` literal, or
a variant reference, bare (`Domestic`) or qualified (`Region.Domestic`).

#### schema_dispatch_clause {#447b5-schema_dispatch_clause}

{{#grammar schema_dispatch_clause}}

`via schema(N)`, `via schema(N..M)`, `via schema(N..)`, `via schema(..M)` and
`via schema(_)`. The argument is a signed number literal, the shared
[`int_range`](#4713-int_range) production, or `_`; that each bound is a positive
`Int`, and that `N <= M`, is well-formedness (§5.7b).

### §4.4.8 by_clause (v0.45) {#448-by_clause}

{{#grammar by_clause}}

`by (<binder>:)? <Actor> ("|" <Actor>)*` — the actor(s) a handler consumes,
positioned after the return type, alongside `given` (v0.155 relocated it from
before the parameter list, where `by <Actor> (…)` read as a call)
(`on schedule("…") () -> … by s: Scheduler`). The **binder is optional** (v0.50):
`by <name>: <Actor>` captures the verified identity (read as `<name>.identity`);
`by <Actor>` declares-and-verifies the contract without capturing it (anonymous
or verify-and-discard). Omitting `by` entirely inherits the protocol's default
actor; on a `from http` handler `by` is required (the binder still optional).

A `|`-separated list of actors (v0.52) is an **ordered sum of peer actors**
(`by who: User | Visitor`): the boundary tries each peer's scheme in declared
order and binds the first that verifies; the body `match`es on the resolved
actor (the binder is **required** for a sum). Well-formedness: §5.

An `actor` is a nominal *boundary contract* — a closed, compiler-known
authentication scheme plus an optional sealed identity — consumed by a handler's
`by` clause (§4.4.8). Actors are context-only.

### §4.4.9 actor_decl (v0.45)

{{#grammar actor_decl}}

`actor <Name> { auth = <Scheme> }`, optionally `, identity = <Type>`. The
refinement form `actor <Name> = <Base> where <predicate>` (v0.53) declares an
authorisation invariant over a `Bearer` base; the predicate is a closed set of
claim predicates (`hasClaim`, `claimEquals`). Well-formedness: §5.

### §4.4.10 scheme

{{#grammar scheme}}

The closed authentication-scheme set. `None`, `Internal`, `Bearer`,
`Signature` (v0.51), `Oidc` (v0.151) and `Foreign` (#1971) are supported. The authenticated schemes
carry a keyed-args config — `Bearer(secret = "<ENV>")`, `Signature(secret =
"<ENV>", header = "<Header>", (timestamp = "<Header>", tolerance = <seconds>)?)`,
`Oidc(issuer = "<url>", audience = "<aud>", jwks = "<url>")` and
`Foreign(secret = "<ENV>", (worker = "<name>")?)` — parsed by the
`scheme_config` production (string- or integer-valued args; the checker validates
which keys each scheme admits). Unlike `Bearer`/`Signature`, an `Oidc` config
names **no secret**: its trust root is the provider's public JWKS. Well-formedness: §5.

### §4.4.10a scheme_config

{{#grammar scheme_config}}

The parenthesised, comma-separated `key = value` arguments an authenticated
scheme carries.

### §4.4.10b scheme_arg

{{#grammar scheme_arg}}

One `key = value` argument; the value is a string or integer literal. Which keys
each scheme admits is well-formedness: §5.

## §4.5 Agents

An `agent` is a keyed, stateful entity whose state lives in `store` fields that
handlers read by name and write with `:=`.

### §4.5.1 agent_decl

{{#grammar agent_decl}}

`agent`, a name, and a body holding a key declaration, `store` fields, zero or
more invariants and transitions, and handlers — in that fixed order.
Well-formedness: §5.

### §4.5.2 key_decl

{{#grammar key_decl}}

`key`, an identifier, `:`, and a type — the agent's identity.

### §4.5.2a store_field

{{#grammar store_field}}

`store`, a name, `:`, a [`store_kind`](#452b-store_kind) over its type parameters,
zero or more [`store_annotation`](#452c-store_annotation)s, and an optional `=`
initialiser — a persistent field of the agent. Well-formedness — required initial
value, kind validity: §5 (ADR 0108).

### §4.5.2b store_kind

{{#grammar store_kind}}

The closed catalogue of storage kinds: `Cell`, `Map`, `Set`, `Cache`, `Log`. The
catalogue is **closed** — there is no `Queue` storage kind (ADR 0122).

### §4.5.2c store_annotation

{{#grammar store_annotation}}

A `@name(args)` annotation between the kind and the initialiser — `@ttl` (on
`Cache`), `@retain` (on `Log`), `@indexed` (on `Map`), `@bounded`. Arguments are
compile-time literals. Well-formedness — kind match, known name: §5 (ADR 0111).

### §4.5.2d annotation_arg

{{#grammar annotation_arg}}

One annotation argument: an optional `label:` and a value expression, as in
`@indexed(by: id)` (labelled) or `@ttl(5.minutes)` (positional). The grammar
admits any expression; the checker restricts arguments to literals and the
`@indexed` field-name labels (ADR 0111 D4).

### §4.5.3 invariant_decl (v0.80)

{{#grammar invariant_decl}}

`invariant`, a name, `:`, and a predicate expression — a universally-quantified
property that must hold of every committed state. Invariants form a phase between
the `store` fields and the handlers; one after a handler is a parse error
(`bynk.parse.invariant_after_handler`). The predicate references the agent's
`store` fields by bare name. Well-formedness — purity, `Bool` type,
agent-locality: §5 (ADR 0107).

### §4.5.4 transition_decl (v0.116)

{{#grammar transition_decl}}

`transition`, a name, `:`, and a predicate expression — a step invariant over the
agent's `old`/`new` state pair. Transitions sit in the same phase as invariants
(between the `store` fields and the handlers); one after a handler is a parse error
(`bynk.parse.transition_after_handler`). The predicate reads state fields through
the contextual `old` and `new` bindings (`old.status`, `new.balance`), which are
ordinary identifiers outside a `transition`. Well-formedness — purity, `Bool` type,
agent-locality, a genuine step reference: §5 (ADR 0151).

### §4.5.5 migrate_decl

{{#grammar migrate_decl}}

`migrate` (a contextual keyword), `from`, a schema version, an optional `(old: T)`
parameter, and a brace-delimited body of `let`, `:=` and `drop` items. Steps sit in
the same phase as invariants (between the `store` fields and the handlers); one after
a handler is a parse error (`bynk.parse.migrate_after_handler`), and any other item
in the body is `bynk.parse.migrate_item`. Well-formedness — the old shape against the
schema lock, coverage, purity, record-literal assignment: §5.4.3.

### §4.5.6 migrate_drop

{{#grammar migrate_drop}}

`drop` (a contextual keyword) and a field name, inside a `migrate` step only: the
step discards that field's stored value on purpose.

### §4.5.7 removed_agent_decl

{{#grammar removed_agent_decl}}

`removed` (a contextual keyword, recognised only before `agent` at context-item
position), `agent`, and a class name. Beside it, `agent_decl` takes an optional
`replaces <Name>` after the agent's name (`replaces` is contextual). Well-formedness
— the old name not live, one fate per name, the schema lock: §5.4.3.

## §4.6 Expressions

Bynk is expression-oriented: a block's value is its final expression. Operator
precedence is fixed by the `binary_expr` production ([§4.6.7](#467-binary_expr)).

### §4.6.1 expression

{{#grammar _expression}}

Any expression: control flow, a refinement check, an operator expression, or a
primary.

### §4.6.2 primary

{{#grammar _primary}}

The atomic and postfix expressions: literals, names, calls, field and method
access, constructors, and parenthesised expressions.

### §4.6.3 if_expr

{{#grammar if_expr}}

`if`, a condition, a block, and an **optional** `else` arm (either a further
`if` or a block). A missing `else` defaults to `()` (v0.146, ADR 0170) and is
legal only when the then-branch is unit (`()` / `Effect[()]`); a value-producing
`if` still requires its `else`. Well-formedness: §5.

### §4.6.4 match_expr

{{#grammar match_expr}}

`match`, a scrutinee, and a brace-delimited list of match arms. Well-formedness —
including exhaustiveness: §5.

### §4.6.5 is_expr

{{#grammar is_expr}}

An expression, `is`, and a pattern. Well-formedness — including the narrowing it
introduces: §5.

### §4.6.6 binary_expr {#467-binary_expr}

{{#grammar binary_expr}}

The binary operators, listed from lowest precedence (`implies`, then `||`) to
highest (`*`, `/`); the production order is the precedence order. `implies`
(v0.80) is logical implication, right-associative, `P implies Q` ≡ `!P || Q`.
Well-formedness: §5.

v0.130 (ADR 0158): a `+`/`-` that begins a new line does **not** continue the
additive chain — it starts a new construct instead, so a comma-less match arm
beginning with a negative literal pattern (`10` ⏎ `-2 => …`) reads as two arms,
not `10 - 2`. This applies only at the `+`/`-` level; `*` and `/` continue
across a line break as usual. Like the same-line `[` rule for type application
([§4.6.8](#468-method_call), [§4.6.10](#4610-call), [§4.6.21a](#4621a-list_literal)),
this is a case where a line break is syntactically significant — see
[§3.4](/book/spec/lexical-grammar/#34-trivia).

### §4.6.7 unary_expr

{{#grammar unary_expr}}

Logical negation `!` and numeric negation `-`, prefixed to an expression.

### §4.6.8 method_call

{{#grammar method_call}}

A receiver, `.`, a method name, and parenthesised arguments. Well-formedness: §5.

v0.22a: the numeric base-type keywords `Int` and `Float` are admitted in
**static-receiver position** — `Int.parse(s)` / `Float.parse(s)` — but only
when immediately followed by `.`; a bare `Int` in expression position remains
a parse error. (`List.empty()` needs no such rule: `List` is lexically an
ordinary identifier.)

v0.22b: a method call accepts **explicit type arguments** —
`Json.decode[Order](s)` — under the same same-line-`[` rule as `call` type
application (0039): a `[` opening a new line is a list literal. In v0.22b
only the `Json.decode` static consumes them; type arguments on any other
method are `bynk.generics.type_arg_mismatch` (generic *user* methods remain
deferred). The bare `name[T]` value form stays reserved.

### §4.6.9 field_access

{{#grammar field_access}}

A receiver, `.`, and a field name. Well-formedness: §5.

### §4.6.9a lambda_expr

{{#grammar lambda_expr}}

A lambda (v0.20a): `(o) => o.paid`, `(acc, t) => acc + t`, `() => 0`, or with
a block body `(o) => { … }`. Always parenthesised; `=>` is the **value**
arrow, shared with `match` arms — `->` stays the type arrow. Well-formedness
(contextual parameter typing, the unannotated rule, bottom-up effectfulness):
§5.

### §4.6.9b lambda_param

{{#grammar lambda_param}}

One lambda parameter: a name with an optional type annotation — optional because
an expected function type supplies it; required in unconstrained positions (§5) —
or, in the name's place, an **irrefutable record pattern** (#2712, patterns track
slice 6), `(Item { qty, .. }) => qty`:

{{#grammar _lambda_record_pattern}}

{{#grammar _lambda_record_field}}

A record pattern is irrefutable when every field is a bare name, `f: name`,
`f: _`, or a nested irrefutable record pattern; a refutable one is
`bynk.parse.lambda_pattern_refutable`, which names the first field that can fail,
and any other pattern (`Some(x)`, `[x, ..]`, `0..9`, a literal) is
`bynk.parse.lambda_pattern_not_record`. A pattern parameter carries no annotation
(`bynk.parse.lambda_pattern_annotation`): the type name states the type. Name and
pattern parameters mix freely, and a pattern counts as one parameter.

The parser desugars `(P) => body` to
`(__lambdaArg<offset>) => { let P = __lambdaArg<offset>; body }`, the `let` being
the `else`-less destructuring of §4.8.4a. The hidden parameter has an empty span
at the pattern and is never printed or named.

### §4.6.10 call

{{#grammar call}}

A name, optional bracketed **type arguments** (v0.20a, `name[T](…)` — the
explicit-instantiation form; a bare `name[T]` without an argument list is a
reserved parse error), and parenthesised arguments — a function call, a
variant construction, an agent instantiation, or (v0.20a) the **application
of a function-typed value** in scope. Well-formedness: §5.

### §4.6.11 record_construction

{{#grammar record_construction}}

A type name and a brace-delimited list of field initialisers. Well-formedness: §5.

### §4.6.12 field_init

{{#grammar field_init}}

One field of a record construction: `name: value`, or the shorthand `name`.

### §4.6.13 record_spread

{{#grammar record_spread}}

A `...` spread of an existing record, optionally overriding fields, with or
without a leading type name. Well-formedness: §5.

### §4.6.14 question_expr

{{#grammar question_expr}}

An expression followed by `?`. Well-formedness: §5.

### §4.6.15 ok_expr

{{#grammar ok_expr}}

`Ok(…)` — the success constructor of `Result` or `HttpResult`. Well-formedness:
§5.

### §4.6.16 err_expr

{{#grammar err_expr}}

`Err(…)` — the failure constructor of `Result`. Well-formedness: §5.

### §4.6.17 some_expr

{{#grammar some_expr}}

`Some(…)` — the present constructor of `Option`. Well-formedness: §5.

### §4.6.18 none_expr

{{#grammar none_expr}}

`None` — the absent constructor of `Option`.

### §4.6.19 effect_pure_expr

{{#grammar effect_pure_expr}}

`Effect.pure(…)` — lifts a pure value into an `Effect`.

### §4.6.20 val_expr

{{#grammar val_expr}}

`Val[T]` with an optional pin argument (v0.114, retiring `Mock[T]`).
Well-formedness — including that it is valid only in test bodies: §5.

### §4.6.21 val_arg

{{#grammar val_arg}}

The pin to a `Val[T]`: positional arguments or a brace-delimited record of field
pins.

### §4.6.21a list_literal

{{#grammar list_literal}}

*(v0.20b)* `[a, b, c]`, with an optional trailing comma — a **leading** `[`
in expression position. It does not collide with explicit type application
(`name[T](…)`, [§4.6.10](#4610-call)): that `[` is a *postfix* form on a
callee identifier and MUST sit on the **same line** as it — a `[` opening a
new line starts a list literal. There is no `Map` literal (`{ }` is records
and blocks) and no indexing form (`get(i)` returns `Option[T]`).
Well-formedness — including empty-literal element-type inference:
[§5.10](/book/spec/static-semantics/#510-collections).

### §4.6.22 paren_expr

{{#grammar paren_expr}}

A parenthesised expression, for grouping.

### §4.6.23 self_expr

{{#grammar self_expr}}

`self` — the receiver inside a method or agent handler. Well-formedness: §5.

### §4.6.24 wire_expr

{{#grammar wire_expr}}

`Wire(<expr>)` — a raw, unvalidated argument to a service address in a
`system`-tier `case` (§4.9): the `String` is the wire form the boundary
receives, a JSON body or a path segment, so a case can drive the router with
input the type system forbids. Its placement is well-formedness:
[§5.9](/book/spec/static-semantics/#59-testing-constructs).

## §4.7 Patterns & matching

The patterns used in `match` arms and `is` checks.

### §4.7.1 match_arm

{{#grammar match_arm}}

A pattern, an optional `if` guard (an arbitrary `Bool` expression over the
pattern's bindings), `=>`, a result expression, and an optional trailing comma —
arm separators are optional. An arm's pattern may also be a refined pattern
([§4.7.10](#4710-refined_pattern)), which only a `match` arm admits: it is not
one of the `_pattern` forms, so it cannot appear after `is` or as an alternative
or sub-pattern. Well-formedness: §5.

### §4.7.2 pattern

{{#grammar _pattern}}

A pattern: a wildcard, a literal, a binding, a variant pattern, a record
pattern ([§4.7.11](#4711-record_pattern)), an or-pattern, or a parenthesised
pattern. A
lowercase-led identifier is a binding (it matches anything and binds the
value); an uppercase-led one is a nullary variant — in the concrete grammar
both parse as `variant_pattern`.

### §4.7.3 variant_pattern

{{#grammar variant_pattern}}

A name, optionally qualified, with an optional parenthesised list of
bindings. The name is an identifier, not a constant name: a lowercase-led
identifier binds the value, an uppercase-led one discriminates a variant.
Well-formedness: §5.

### §4.7.4 wildcard_pattern

{{#grammar wildcard_pattern}}

`_` — matches anything and binds nothing.

### §4.7.5 pattern_binding

{{#grammar _pattern_binding}}

A binding within a variant pattern: a named binding, or a full sub-pattern
matched against the payload field (one recursion point). A lowercase-led
identifier binds the field, an uppercase-led one discriminates a nested nullary
variant (`Err(PollClosed)`), `_` ignores it, and a nested variant recurses
(`Some(Ok(x))`). Well-formedness: §5.

### §4.7.6 named_binding

{{#grammar named_binding}}

Binds a payload field by name, matching it against a sub-pattern: `field: name`,
or `field: _` to ignore it.

### §4.7.7 literal_pattern

{{#grammar literal_pattern}}

An integer (optionally negated), a string, or a boolean, matching a primitive
scrutinee by value. Exhaustiveness: [§5.6](/book/spec/static-semantics/#56-pattern-matching).

### §4.7.8 or_pattern

{{#grammar or_pattern}}

`p₁ | p₂`, left-associative: matches if either alternative matches. `|` here is
a pattern operator, distinct from boolean `||`. That every alternative binds the
same names at the same types, against one value type, is well-formedness:
[§5.6](/book/spec/static-semantics/#56-pattern-matching).

### §4.7.9 paren_pattern

{{#grammar paren_pattern}}

A pattern in parentheses: transparent grouping, most useful around an
or-pattern. It never admits a refined pattern inside.

### §4.7.10 refined_pattern

{{#grammar refined_pattern}}

`_ where predicate` — a runtime guard on a `match` arm, reusing the
refinement-predicate catalogue of [§4.2.11](#4211-refinement). The inner form is
the wildcard only: any other inner pattern, `(p₁ | p₂)` included, is
`bynk.parse.refined_pattern_inner`. Its scrutinee types and exhaustiveness are
well-formedness: [§5.6](/book/spec/static-semantics/#56-pattern-matching).

### §4.7.11 record_pattern {#4711-record_pattern}

{{#grammar record_pattern}}

`TypeName { f1, f2: pattern, .. }` (patterns track, slice 1, #2641) — matches a
value of a record type by its fields. The type name is required and carries no
type arguments: a generic record's come from the scrutinee. A bare field `f`
binds that field to a local named `f`; `f: p` matches the field against the
sub-pattern `p` (any pattern, a record pattern included).

Record patterns are **always open**: the trailing `..` is required, and fields
not listed are ignored. Adding a field to a record therefore never breaks a
record pattern. A pattern without the `..` is `bynk.parse.record_pattern_without_rest`.

{{#grammar record_pattern_field}}

After `is`, a record pattern MUST be parenthesised —
`x is (T { f, .. })` — because there the `{` would also open the `if`'s block,
so the pattern admitted without parentheses is the same as `_pattern` less a
record pattern at its tail, at the top level or as an or-alternative:

{{#grammar _is_pattern}}

{{#grammar _is_base_pattern}}

{{#grammar _is_or_pattern}}

An unparenthesised one is `bynk.parse.record_pattern_in_is_unparenthesised`. A
record pattern inside a delimiter — a variant payload (`x is Some(T { f, .. })`)
or parentheses — needs nothing more. Well-formedness: §5.6.

### §4.7.12 list_pattern {#4712-list_pattern}

{{#grammar list_pattern}}

{{#grammar list_rest}}

`[]`, `[p₁, …, pₙ]`, `[p₁, …, pₙ, ..]` and `[p₁, …, pₙ, ..rest]` (patterns
track, slice 4, #2680) — match a `List` by its length and elements. Without a
rest marker the pattern matches a list of exactly *n* elements, element *i*
against `pᵢ`. With the rest marker `..` it matches *n* or more elements; a name
after it (`..rest`) binds the elements from the *n*th on, and `..` alone binds
nothing. *n* may be 0: `[..]` matches every list, and `[..rest]` binds the whole
list. An element is any pattern, so list patterns nest (`Some([x, ..])`,
`[[a, ..], ..]`).

The rest marker `..` is the rule it is in a record pattern, with one more case,
shared with range patterns (patterns track, Q7). After `..` the next token
decides:

- a lowercase name, `,` before the closing `]`, or `]` makes it a **rest marker**;
  a capitalised name is rejected, since it would be a named constant bounding a
  range (`..MAX`);
- an integer, including a negative one (`..9`, `..-1`), is the upper bound of a
  range pattern ([§4.7.13](#4713-int_range)), an element of the list.

The rest marker MUST be the final element, and appears at most once; a `..` with
anything after it is `bynk.parse.list_rest_not_last` (suffix patterns such as
`[.., last]` are not available). `.._` is not a form, because a bare `..` already
binds nothing. A `[` begins no other pattern, so a list pattern needs no
parentheses anywhere, after `is` included: the pattern closes at the `]`. It may
begin a `let` (§4.8.4a). Well-formedness: §5.6.

### §4.7.13 int_range {#4713-int_range}

{{#grammar int_range}}

`lo..hi`, `lo..` and `..hi` (patterns track, slice 5, #2699; events slice 4b,
#990) — an inclusive integer range, each bound an optionally negative number
literal. It is one production, used by a match pattern and by `via schema(...)`
(§4.4.7).

A bare `..` is never a range. Whether `..` is a rest marker or a range is decided
by the next token alone (§4.7.12): a range has a number on at least one side, and
a rest marker is never followed by one. The high bound of `lo..` is read greedily,
so `5.. -1` is the range `5..-1`; an open range ends where the next token cannot
be a number, which in a `match` arm is always the `=>`, `if`, `|`, `,` or closing
delimiter that follows a pattern. `0..9` lexes as three tokens, never a float,
because a float needs a digit on both sides of its `.`.

A range opens a match arm, a list element, a variant payload and a `let` pattern,
and is an or-pattern alternative. After `is` it parses and is rejected by the
checker (`bynk.types.is_range_pattern`); the tree-sitter grammar does not admit it
there, since an arm body ending `x is 5` and the next arm starting `..9` would
otherwise be one range. A bound's range and `lo <= hi` are well-formedness (§5.6).

## §4.8 Statements

A block is a sequence of statements ending in an optional value expression.

### §4.8.1 block

{{#grammar block}}

A brace-delimited sequence of statements with an optional trailing expression,
which is the block's value. A block with **no** trailing expression has the
implicit value `()` (unit) — the parser synthesises a `()` tail (v0.146, ADR
0170); an empty block `{}` is therefore the unit value.

### §4.8.2 statement

{{#grammar _statement}}

A statement: a `let`, an effectful `let`, an asynchronous send (`~>`), a `do`
effect statement, a `:=` store write, a `par` block, or an assertion.

### §4.8.3 let_stmt

{{#grammar let_stmt}}

`let`, a binding name, an optional type annotation, `=`, and an expression.
Well-formedness: §5.

### §4.8.4 effect_let_stmt

{{#grammar effect_let_stmt}}

`let`, a binding name, an optional type annotation, `<-`, and an effect
expression. Well-formedness: §5.

### §4.8.4a let_else_stmt (#2010)

{{#grammar let_else_stmt}}

`let`, a pattern, `=` or `<-`, a value, an optional call-site actor clause
([§4.9.6](#496-call_site_actor-v0182)), `else`, and either a block or a
`let_else_match` (the `else` may be omitted for an irrefutable record pattern,
below):

{{#grammar let_else_match}}

The pattern MUST open as a pattern rather than a name: with a literal, with `(`,
with `Ok`/`Err`/`Some`/`None`, with a variant name followed by `(` or `.`
(`Shipped(at)`, `Status.Done`), or with a capitalised name followed by `{`, which
opens a record pattern ([§4.7.11](#4711-record_pattern); #2654), or with `[`, which
opens a list pattern ([§4.7.12](#4712-list_pattern); #2680), or with a range
([§4.7.13](#4713-int_range); #2699), which begins with a number followed by `..`,
or with `..` followed by a number:

{{#grammar _let_pattern}}

{{#grammar _let_variant_pattern}}

An or-pattern after `let` opens the same way, its later alternatives being any
pattern:

{{#grammar _let_or_pattern}}

The `else`-less form of a record `let` (below) takes its record pattern bare or
in parentheses; the parenthesised one is:

{{#grammar _let_paren_record_pattern}}

A bare name after `let`, capitalised or not, is always a binding (§4.8.3), since
a binding name is never followed by `{`. A nullary variant is written qualified
(`let Status.Done = s else …`) or in parentheses (`let (Done) = s else …`). A
pattern `let` carries no type annotation (`bynk.parse.let_pattern_annotation`)
and MUST have its `else`, with one exception: a `let` whose pattern is an
**irrefutable record pattern** may omit it, `let Item { sku, qty, .. } = item`
(#2654, patterns track slice 2). A record pattern is irrefutable when every field
is a bare name, `f: name`, `f: _`, or a nested irrefutable record pattern;
otherwise the `else` is required and its absence is
`bynk.parse.let_pattern_without_else`, naming the first field that can fail to
match. Parentheses around the pattern are permitted and redundant
(`let (Item { sku, .. }) = item`), and no other pattern gains the `else`-less
form. An `else` after a plain binding is
`bynk.parse.let_else_on_binding`. If the value ends in an `if` with no `else`,
the `else` belongs to that `if`, so parenthesise such a value. Meaning and
well-formedness: §5.6.

### §4.8.5 effect_send_stmt

{{#grammar effect_send_stmt}}

`~>` and an effect expression — an **asynchronous send**. Unlike an
`effect_let_stmt` it carries **no binder**: the reply is not awaited and nothing
is bound. Well-formedness — including the requirement that the reply be
`Effect[()]` (the error gate): §5.

### §4.8.6 do_stmt (v0.146)

{{#grammar do_stmt}}

`do` and an effect expression — a **unit effect statement** (ADR 0170). Like an
`effect_send_stmt` it carries **no binder**; unlike it, the effect *is* awaited
and joins the enclosing computation. The binder-free spelling of `let _ <- e`
when the awaited value is unit. Well-formedness — including the requirement that
the operand be `Effect[()]` (a valued reply keeps the explicit `let _ <- e`): §5.

### §4.8.7 assign_stmt (v0.81)

{{#grammar assign_stmt}}

An identifier, `:=`, and an expression — a `Cell` store write. Well-formedness —
including that the target is a `store Cell` field and the right-hand side does not
read it: §5 (ADR 0108).

### §4.8.7a advance_stmt (#2137)

{{#grammar advance_stmt}}

The contextual word `advance`, a capability name, `by`, and an expression — a
move of the test case's harness clock. `advance` opens a statement only before
`<Ident> by`. Well-formedness — only in a `case` body, only on `Clock`, by a
`Duration`: §5.

### §4.8.7b par_stmt (#2640)

{{#grammar par_stmt}}

{{#grammar _par_statement}}

The contextual word `par`, `{`, statements, `}` — a **concurrent binding block**
(concurrent effects slice 0, #2640). `par` opens a statement only before `{`;
anywhere else it is an ordinary identifier, so no existing name is taken. The
grammar admits every statement but a `let … else` (which would take the rest of
the block as its arm, and so is `bynk.par.invalid_statement` at parse time); the
checker narrows the contents to `let x <- e` and `do e`. Well-formedness and
meaning: [§5.5](/book/spec/static-semantics/#55-effects-capabilities--providers).

### §4.8.8 expect_expr

{{#grammar expect_expr}}

`expect` and a `Bool` predicate, or — inside a `case` — an `observation_expr`
or a `faults_expr`. Well-formedness: §5.

### §4.8.9 observation_expr (v0.117)

{{#grammar observation_expr}}

An observation over a capability seam: a `Cap.op` subject (the capability and one
of its operations, *named, not called*) followed by a matcher — `called`
(optionally `once` / `<n> times`, optionally `with <predicate>`), `never called`,
or `before Cap.op`. The matcher words `called`, `never`, `once`, `times`, `with`,
`before` are contextual: outside an observation clause they are ordinary
identifiers. A `with` predicate reads the operation's parameters by their declared
names. Well-formedness — seam resolution, `with` scope and purity: §5.

### §4.8.9a faults_expr

{{#grammar faults_expr}}

A fault claim (#1706): an effectful call followed by `faults`, claiming that
awaiting the call throws rather than returns. `faults` is contextual: outside an
`expect` subject it is an ordinary identifier. Well-formedness — the subject is a
call of type `Effect[_]`, and the claim is not made at the `system` tier: §5.

### §4.8.9b trace_expr (v0.117)

{{#grammar trace_expr}}

`trace(Cap.op)` — a test-only builtin yielding the recorded calls of a capability
operation as a `List` of per-operation call records, in call order. `trace` is an
ordinary identifier outside a `case`. Well-formedness: §5.

### §4.8.9c contextual_identifier

{{#grammar _contextual_identifier}}

The contextual words `trace` and `advance` in identifier position. Each is a
keyword only in its own shape (`trace(Cap.op)`, §4.8.9b; `advance Clock by …`,
`advance_stmt`); anywhere else an identifier is admitted — a primary
expression, a call's callee, a lambda parameter, an assignment target — it is an
ordinary identifier.

### §4.8.10 binding_name

{{#grammar _binding_name}}

The name bound by a `let`: an identifier, or `_` to discard the value.

## §4.9 Testing constructs

Cases, tiers, and provider overrides. See also the top-level `suite_decl`
([§4.1.7](#416-test_decl)).

### §4.9.1 case

{{#grammar case}}

`case`, a description string, an optional `as <tier>` clause (`unit` |
`integration` | `system`; `unit` is the default and elided), and a block body
whose leading items may be case-scoped `stub` and `env` clauses. Well-formedness: §5.

### §4.9.2 stub_clause

{{#grammar stub_clause}}

A per-seam provider override: `stub`, a capability, `.`, a method, a
parenthesised argument-pattern list (`_` or a value per parameter), and a
right-hand side — `returns <value>` (optionally followed by `after <duration>`,
#2683), `returns each [<outcome>, …]`, `fails`, or `never`. `after` and `never`
are contextual. Legal at suite and case scope. Well-formedness: §5.

### §4.9.2a env_clause

{{#grammar env_clause}}

{{#grammar env_entry}}

A suite's configuration: `env` and a braced list of `NAME = <value>` entries,
separated by newlines or commas. Legal at suite and case scope, where it leads
the case body with any `stub` clauses. `env` is a contextual word, recognised
only when a `{` follows it there. Well-formedness:
[§5](/book/spec/static-semantics/#suite-env).

### §4.9.3 property_decl (v0.114)

{{#grammar property_decl}}

`property`, a description string, and a block holding one `for all` binder: the
generative sibling of `case`, whose subjects the runner produces.
Well-formedness: [§5.9a](/book/spec/static-semantics/#59a-generative-properties).

### §4.9.4 for_all

{{#grammar for_all}}

`for all`, one or more comma-separated bindings, an optional `where` filter
expression, and a block body of `expect`s.

### §4.9.5 for_all_binding

{{#grammar for_all_binding}}

`name: Type` — binds `name` to a generated inhabitant of `Type`.

### §4.9.6 call_site_actor (v0.182)

{{#grammar call_site_actor}}

`by <Actor>` or `by <Actor>(<identity>)` in a `case` body: the actor the case
acts as when it drives a service handler, with its identity value. Distinct
from the handler's [`by_clause`](#448-by_clause), which binds an actor and
carries no identity argument. Well-formedness: §5.9.
