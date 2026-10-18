---
title: Syntax & grammar
---
The annotated grammar reference: every Bynk construct, with its production, what
it means, the diagnostics that govern it, and an example. The verbatim machine
grammar — every production in one block — is the
[complete grammar appendix](/book/reference/grammar-appendix/).

## Notation & conventions

Productions are written in EBNF:

`"x"` a literal token · `/x/` a regular expression · `( … )?` optional ·
`( … )*` zero or more · `( … )+` one or more · `a | b` choice · `ε` empty.

- **Nonterminals** are the unquoted names; each is defined by its own entry on
  this page. Names are the *display* names of the grammar rules: a leading
  underscore (an internal helper rule) is dropped and trivial wrappers are
  collapsed, so productions read as language rather than parser internals. The
  raw rules and the byte-exact grammar live in the
  [appendix](/book/reference/grammar-appendix/).
- Every production on this page is **generated** from the `tree-sitter-bynk`
  grammar, so it cannot drift from that grammar; a cross-parser conformance test
  keeps that grammar in agreement with the compiler's own parser.
- A production says what *parses*. A **Static semantics** block lists the
  `bynk.*` diagnostics that constrain a construct beyond parsing; each links by
  code to the [diagnostic index](/book/reference/diagnostics/). A construct with no such
  diagnostics says so.

## Lexical grammar

The terminals: identifiers, literals, comments, and the trivia ignored between
tokens.

### identifier {#rule-identifier}

{{#grammar identifier}}

A name: a letter followed by letters, digits, or underscores. Used for
declarations, parameters, fields, and bindings.

**Static semantics.**
{{#grammar-semantics identifier}}

### constant_name {#rule-constant_name}

{{#grammar constant_name}}

An upper-case-initial name, used for sum-type variants and enum constants.

### number_literal {#rule-number_literal}

{{#grammar number_literal}}

A non-negative integer literal.

**Static semantics.**
{{#grammar-semantics number_literal}}

### float_literal {#rule-float_literal}

{{#grammar float_literal}}

A `Float` literal: a fraction with a digit required on both sides of the `.`
(`1.0`, `0.5` — `1.` and `.5` are rejected), an exponent (`1e10`, `1.5e-3`),
or both. A literal that does not fit a finite 64-bit float (`1e999`) is
rejected at lex time.

**Static semantics.**
{{#grammar-semantics float_literal}}

### string_literal {#rule-string_literal}

{{#grammar string_literal}}

A double-quoted string. The escapes `\n`, `\t`, `\"`, and `\\` are recognised.
A string may also contain `\(expr)` interpolation holes (v0.43); see
`string_interpolation`.

**Static semantics.**
{{#grammar-semantics string_literal}}

### string_interpolation {#rule-string_interpolation}

{{#grammar string_interpolation}}

An interpolation hole `\(expr)` inside a string literal (v0.43). The body is an
ordinary expression; the hole's parentheses balance, so `\(f(x))` takes `f(x)`.
A bare `\(` was previously an invalid escape, so this is backward-compatible
(`\\(` is an escaped backslash followed by a literal `(`). The hole-typing rule
is in [§5.2 well-typedness](/book/spec/static-semantics/#52-well-typedness).

### boolean_literal {#rule-boolean_literal}

{{#grammar boolean_literal}}

The two `Bool` values, `true` and `false`.

### unit_literal {#rule-unit_literal}

{{#grammar unit_literal}}

The unit value `()` — the single value of the unit type.

### line_comment {#rule-line_comment}

{{#grammar line_comment}}

A comment from `--` to end of line. Bynk uses `--`, never `//`. Comments are
trivia: ignored between tokens. A `--` opens a comment only at the start of input
or when **preceded by whitespace** — adjacent to a token, `a--b` is `a - -b`
(subtraction), not a comment (spec [§3.3.1](/book/spec/lexical-grammar/#331-line_comment)).

A `--- … ---` **doc-block** is an external token attached to the following
declaration; its markers are lines of **three or more** hyphens, and the closing
marker has **as many hyphens as the opening one**, so a `----` block can hold
`---` lines. There is no standalone `---` divider (an unclosed marker is
`bynk.lex.unclosed_doc_block`), and a block whose content, outside Markdown code
fences, parses as declarations is `bynk.parse.doc_block_contains_code`. A
doc-block, doc lines, whitespace, and line comments are the trivia ignored
between tokens (see the appendix's [Tokens & trivia](/book/reference/grammar-appendix/#tokens--trivia)).

**See also.** [Keywords](/book/reference/keywords/).

### doc_line {#rule-doc_line}

{{#grammar doc_line}}

The one-line doc form (#1888). A `--|` that is the **first thing on its line**
opens a doc line; after code on the same line it is an ordinary `--` comment.
Doc lines on consecutive lines form one doc, joined with line breaks, so a bare
`--|` separates Markdown paragraphs. A doc of either form, a run of `--|` lines
or a `---` block, documents the declaration below it and also a record field, a
sum variant, a variant's payload field or a `messages` entry
(spec [§3.3.3](/book/spec/lexical-grammar/#333-doc_line)).

```bynk
--| A full commit SHA.
type Commit = String

type Outcome = enum {
	--| Every canary check passed.
	Pass,
	Fail,
}
```

A declaration or member carries one form, not both
(`bynk.parse.doc_forms_mixed`). For which form to use, see
[Document your code](/book/guides/program-structure/document-your-code/).

## Top-level & modules

A source file is a `commons` (pure, shareable code) or a `context` (an isolated
bounded context), or test declarations. The helper *fragment* rules are entry
points the tooling uses to parse incomplete input; they are not written
directly.

### source_file {#rule-source_file}

{{#grammar source_file}}

A whole file: one or more top-level declarations, or a fragment (used by editor
tooling).

**Example.**
```bynk
commons shop {
  type Status =
    | Pending
    | Shipped(tracking: String)
    | Cancelled(reason: String)

  fn describe(s: Status) -> String {
    match s {
      Pending => "awaiting shipment"
      Shipped(tracking: t) => t
      Cancelled(reason: r) => r
    }
  }
}
```

**See also.** [How a Bynk program is shaped](/book/guides/program-structure/how-a-program-is-shaped/) · [Lay out a project](/book/guides/projects-build-and-deployment/layout/).

### item_fragment {#rule-_item_fragment}

{{#grammar _item_fragment}}

A tooling entry point: a single body item parsed in isolation. Not written by
hand.

### expr_fragment {#rule-_expr_fragment}

{{#grammar _expr_fragment}}

A tooling entry point: statements and/or an expression parsed in isolation. Not
written by hand.

### commons_decl {#rule-commons_decl}

{{#grammar commons_decl}}

A `commons` module: pure, dependency-free declarations (types, functions,
capabilities) shareable across contexts. Body braces are optional at file scope.

### context_decl {#rule-context_decl}

{{#grammar context_decl}}

A `context`: a bounded context with its own services, agents, and provided
capabilities, isolated behind its boundary.

**Example.**
```bynk
context reaper

service sweeper from cron {
  on schedule("*/5 * * * *") (at: Int) -> Effect[Result[(), String]] {
    Ok(())
  }
}
```

**Static semantics.**
{{#grammar-semantics context_decl}}

**See also.** [How a Bynk program is shaped](/book/guides/program-structure/how-a-program-is-shaped/).

### adapter_decl {#rule-adapter_decl}

{{#grammar adapter_decl}}

An `adapter`: the host boundary. It co-locates a capability contract with a
non-Bynk `binding`, declaring capabilities, boundary types, inline pure helpers,
and external (bodiless) providers. The only place host code may enter a program.

**Example.**
```bynk,fragment
adapter tokens {
  binding "./tokens.binding.ts" requires { "jose": "^5" }
  exports capability  { Jwt }
  exports transparent { Claims }
  type Claims = { sub: String, exp: Int }
  capability Jwt {
    fn sign(claims: Claims, secret: String) -> Effect[String]
  }
  provides Jwt = JoseJwt
}
```

**Static semantics.**
{{#grammar-semantics adapter_decl}}

**See also.** [Adapters](/book/reference/adapters/) · [Wrap a library as an adapter](/book/guides/effects-and-capabilities/wrap-a-library/).

### suite_decl {#rule-suite_decl}

{{#grammar suite_decl}}

A `suite` block targeting a `commons` or `context`, holding its `case`s and
`stub` clauses. An optional `as <tier>` classifier — `unit`, `integration`, or
`system` — records the test level (the tier words are contextual, not keywords).

**Static semantics.**
{{#grammar-semantics suite_decl}}

**See also.** [Testing](/book/reference/testing/) · [Write tests and stub collaborators](/book/guides/testing/write-tests/).

### commons_body_item {#rule-_commons_body_item}

{{#grammar _commons_body_item}}

The declarations allowed in a `commons` (no `consumes` or `exports`).

### context_body_item {#rule-_context_body_item}

{{#grammar _context_body_item}}

The declarations allowed in a `context`, including `consumes` and `exports`.

### adapter_body_item {#rule-_adapter_body_item}

{{#grammar _adapter_body_item}}

The declarations allowed in an `adapter`: a `binding` clause, capabilities,
boundary types, inline pure helpers and `uses`, external providers, and
`exports` (no `consumes`).

### test_body_item {#rule-_test_body_item}

{{#grammar _test_body_item}}

The declarations allowed in a `suite` block, including `stub` and `env` clauses and
`case`s.

### qualified_name {#rule-qualified_name}

{{#grammar qualified_name}}

A dotted name, e.g. `shop.orders` — used to name modules and reference them.

### uses_decl {#rule-uses_decl}

{{#grammar uses_decl}}

Imports a `commons` so its public names are in scope.

**Static semantics.**
{{#grammar-semantics uses_decl}}

**See also.** [Define sum, record, and opaque types](/book/guides/type-system/define-types/).

### consumes_decl {#rule-consumes_decl}

{{#grammar consumes_decl}}

Declares that a context depends on another context's (or adapter's) services or
capabilities — whole and qualified, aliased (`as`), or with selected
capabilities flattened to bare names (`{ Cap, … }`).

A context's `consumes` clauses apply to all its files. In a context split across
files, each file may state what it consumes, repeating an aliased clause or a
`{ Cap }` selection another file states; the same clause twice in one file is
still an error. An alias names one unit, and a unit is consumed under one alias:
`consumes shop.payment as Payment` in one file and `as Pay` in another is
`bynk.consumes.alias_conflict`. A unit may also be consumed unaliased, by its
qualified name, beside its alias.

**Static semantics.**
{{#grammar-semantics consumes_decl}}

**See also.** [Consume another context's services](/book/guides/program-structure/consume-services/).

### binding_decl {#rule-binding_decl}

{{#grammar binding_decl}}

Names an adapter's TypeScript binding module (resolved relative to the adapter's
source file) and, optionally, the npm dependencies it requires. Pinned version
ranges only.

**Static semantics.**
{{#grammar-semantics binding_decl}}

**See also.** [Adapters](/book/reference/adapters/).

### binding_requirement {#rule-binding_requirement}

{{#grammar binding_requirement}}

One `"package": "range"` entry in a binding's `requires { … }` map; folded into
the generated `package.json`.

### exports_decl {#rule-exports_decl}

{{#grammar exports_decl}}

Controls a context's boundary: which types are exported opaquely or
transparently, and which capabilities are exported.

**Static semantics.**
{{#grammar-semantics exports_decl}}

**See also.** [Share a capability across contexts](/book/guides/effects-and-capabilities/share-across-contexts/).

## Types & refinements

Type declarations and the type references that appear in signatures.

### type_decl {#rule-type_decl}

{{#grammar type_decl}}

Names a type as a record, sum, enum, opaque, or refined type.

**Example.**
```bynk
type Status =
  | Pending
  | Shipped(tracking: String)
  | Cancelled(reason: String)
```

**Static semantics.**
{{#grammar-semantics type_decl}}

**See also.** [Type system](/book/reference/types/) · [Define sum, record, and opaque types](/book/guides/type-system/define-types/) · [The type-system philosophy](/book/guides/type-system/philosophy/).

### type_body {#rule-_type_body}

{{#grammar _type_body}}

The right-hand side of a `type` declaration: one of the five type forms.

### opaque_type {#rule-opaque_type}

{{#grammar opaque_type}}

A type whose representation is hidden outside its defining module; constructed
and inspected only through its API.

**See also.** [Define sum, record, and opaque types](/book/guides/type-system/define-types/).

### refined_type {#rule-refined_type}

{{#grammar refined_type}}

A base or named type narrowed by a `where` refinement, e.g. `Int where
Positive`.

**Example.**
```bynk
type Quantity = Int where InRange(1, 100)
```

**Static semantics.**
{{#grammar-semantics refined_type}}

**See also.** [Refined-type API](/book/reference/refined-types/) · [Define and validate untrusted input](/book/guides/type-system/define-and-validate/).

### record_type {#rule-record_type}

{{#grammar record_type}}

A product type: named fields, each with a type and optional refinement and
default.

**Static semantics.**
{{#grammar-semantics record_type}}

### record_field {#rule-record_field}

{{#grammar record_field}}

One field of a record: a name, a type, an optional inline refinement, and an
optional default value.

**Static semantics.**
{{#grammar-semantics record_field}}

### sum_type {#rule-sum_type}

{{#grammar sum_type}}

A tagged union of variants, each optionally carrying a payload.

**Static semantics.**
{{#grammar-semantics sum_type}}

**See also.** [Type system](/book/reference/types/).

### sum_variant {#rule-sum_variant}

{{#grammar sum_variant}}

One variant of a sum type: a constant name with an optional payload.

### variant_payload_field {#rule-variant_payload_field}

{{#grammar variant_payload_field}}

A named field in a sum-variant payload.

### enum_type {#rule-enum_type}

{{#grammar enum_type}}

A sum type whose variants all have no payload.

### refinement {#rule-refinement}

{{#grammar refinement}}

One or more predicates joined by `&&`, narrowing a **type** to the values that
satisfy them. This is the closed **type-refinement catalogue** — a fixed set of
built-in predicates ([`predicate_name`](#rule-predicate_name)), not user
expressions.

> **The three `where` / predicate tiers.** `where` (and the other predicate
> positions) look uniform but come in three tiers with different vocabularies:
>
> 1. **Type refinement** — `type T = Base where <catalogue>`
>    ([`refined_type`](#rule-refined_type), this rule). A *closed grammar*: the
>    built-in `predicate_name`s joined by `&&`. No user expressions.
> 2. **Actor claim** — `actor A = Base where <predicate>`
>    ([`actor_decl`](#rule-actor_decl)). A full expression that a static-semantics
>    rule restricts to the closed **actor-claim catalogue** (`hasClaim` /
>    `claimEquals` composed with `&&` / `||` / `!`).
> 3. **Boolean contracts & tests** — a function `requires` / `ensures`, an agent
>    `invariant` / `transition`, a test `expect` (ADR 0144's "one predicate
>    surface"). The *open* tier: any pure `Bool` expression over `is` / `implies`
>    / the operators / pure value methods.
>
> Tier 1 is closed in the *grammar*; tier 2 is closed by *static semantics* over a
> parsed expression; tier 3 is open. ADR 0144's "one predicate surface" names
> tier 3 — the two closed catalogues (1 and 2) are deliberately narrower.

**Static semantics.**
{{#grammar-semantics refinement}}

**See also.** [The refined-literal admission model](/book/guides/type-system/refined-literal-admission/).

### refinement_pred {#rule-_refinement_pred}

{{#grammar _refinement_pred}}

A single refinement predicate: a predicate call or a bare predicate.

### pred_call {#rule-pred_call}

{{#grammar pred_call}}

A predicate with arguments, e.g. `InRange(1, 100)` or `Matches("…")`.

### predicate_name {#rule-predicate_name}

{{#grammar predicate_name}}

The built-in refinement predicates.

**Static semantics.**
{{#grammar-semantics predicate_name}}

### pred_arg {#rule-_pred_arg}

{{#grammar _pred_arg}}

An argument to a predicate: a number or string literal.

### base_type {#rule-base_type}

{{#grammar base_type}}

The primitive types `Int`, `String`, `Bool`, `Float`, and `Duration`. `Duration`
(v0.86, ADR 0112) is a span of time in milliseconds, written with a literal
`<int>.<unit>` (`5.minutes`, `30.days`); its closed unit set is `milliseconds`,
`seconds`, `minutes`, `hours`, `days`.

**Static semantics.**
{{#grammar-semantics base_type}}

### type_ref {#rule-_type_ref}

{{#grammar _type_ref}}

A type as it appears in a signature: a base type, a unit, a validation-error
type, a generic application, or a named type.

### unit_type {#rule-unit_type}

{{#grammar unit_type}}

The unit type `()`.

### validation_error_type {#rule-validation_error_type}

{{#grammar validation_error_type}}

`ValidationError` — the error produced when refined-type validation fails.

### generic_type_ref {#rule-generic_type_ref}

{{#grammar generic_type_ref}}

A generic type applied to arguments: `Result[T, E]`, `Option[T]`, `Effect[T]`,
`HttpResult[T]`, or `Stream[T]`.

**Static semantics.**
{{#grammar-semantics generic_type_ref}}

**See also.** [Work with `Result` and optional values](/book/guides/type-system/result-and-optionals/).

### applied_type_ref {#rule-applied_type_ref}

{{#grammar applied_type_ref}}

An application of a **user-declared generic type** (v0.157) to bracketed type
arguments: `Paginated[User]`, `Keyed[String, Int]`. The head is a user type
name declared `type Name[T, …] = { … }`; the built-in generics have their own
[`generic_type_ref`](#rule-generic_type_ref). A generic type must be applied to
exactly its declared number of arguments, and a generic record is a
**non-boundary** value — it cannot appear in a record field, sum payload,
handler signature, agent state, or JSON codec target.

**Static semantics.**
{{#grammar-semantics applied_type_ref}}

### function_type_ref {#rule-function_type_ref}

{{#grammar function_type_ref}}

A function type (v0.20a): `Int -> Int`, `(Int, String) -> Bool`, `() -> Int`.
The arrow is **right-associative** (`A -> B -> C` is `A -> (B -> C)`), and a
function type is effectful exactly when its return type is `Effect[_]` — the
same structural rule that classifies function declarations. Function types are
confined to **non-boundary** positions: fn/lambda parameters, returns, and
locals; they are rejected in record fields, sum payloads, handler and
capability signatures, agent state, and anything else that would serialise or
cross a boundary.

**Static semantics.**
{{#grammar-semantics function_type_ref}}

## Functions, capabilities & providers

Pure functions and methods, the capability interfaces an effectful program
depends on, and the providers that implement them.

### fn_decl {#rule-fn_decl}

{{#grammar fn_decl}}

A function or method: a name, parameters, a return type, and a block body.

**Example.**
```bynk
commons demo {
  type Id = Int

  fn add(a: Int, b: Int) -> Int {
    a + b
  }
}
```

**Static semantics.**
{{#grammar-semantics fn_decl}}

**See also.** [Operators & built-ins](/book/reference/operators/).

### requires_clause {#rule-requires_clause}

{{#grammar requires_clause}}

A function **precondition** (v0.115): `requires <name>: <predicate>`, between the
return type and the body. A pure `Bool` predicate over the parameters — the one
predicate surface. Checked at every call in the dev/test build and used to filter
the runner's generated arguments; `result` is not in scope
(`bynk.contract.result_in_requires`).

**See also.** [Contracts](/book/reference/testing/#contracts).

### ensures_clause {#rule-ensures_clause}

{{#grammar ensures_clause}}

A function **postcondition** (v0.115): `ensures <name>: <predicate>`. A pure
`Bool` predicate over the parameters **and** `result` (the return value; the
awaited element for an `Effect`). Checked at every call in the dev/test build and
generated against by the runner (a contract is a property that is always on). A
`case`/`property` that merely restates it is flagged
(`bynk.contract.restated_by_test`).

**See also.** [Contracts](/book/reference/testing/#contracts).

### method_name {#rule-method_name}

{{#grammar method_name}}

A method name, `Type.method`, defining a method on a named type.

### params {#rule-_params}

{{#grammar _params}}

A parameter list: an optional `self` parameter followed by named parameters.

### self_param {#rule-self_param}

{{#grammar self_param}}

The `self` receiver of a method or handler.

### param {#rule-param}

{{#grammar param}}

One parameter: a name and a type.

**Static semantics.**
{{#grammar-semantics param}}

### capability_decl {#rule-capability_decl}

{{#grammar capability_decl}}

A capability: an interface of effectful operations a context can depend on.

**Example.**
```bynk
context demo

capability Logger  { fn info(message: String) -> Effect[()] }
capability Greeter { fn greet() -> Effect[()] }

provides Logger = ConsoleLogger {
  fn info(message: String) -> Effect[()] {
    Effect.pure(())
  }
}

provides Greeter = PoliteGreeter given Logger {
  fn greet() -> Effect[()] {
    let _ <- Logger.info("hello")
    Effect.pure(())
  }
}
```

**Static semantics.**
{{#grammar-semantics capability_decl}}

**See also.** [Capabilities & providers](/book/reference/capabilities/).

### capability_op {#rule-capability_op}

{{#grammar capability_op}}

One operation in a capability: a name, parameters, and a return type (no body).

**Static semantics.**
{{#grammar-semantics capability_op}}

### messages_decl {#rule-messages_decl}

{{#grammar messages_decl}}

A message bundle for one locale (message-bundles track, slice 1): `messages
"<tag>" @reference { "code" => "template" ... }`. `tag` is a string literal
(v0.233) — its `LocaleTag` refinement is a checker concern, not a grammar one.
Annotations reuse the same `@name(args)` shape as a `store` field's
(`store_annotation`); the parser stays permissive on their count (`@reference`
appears zero or more times syntactically) — exactly one per bundle is a
checker rule (`bynk.messages.missing_reference` / `multiple_reference`), not a
parse error. Commons-only placement is likewise checked, not parsed: this rule
is syntactically admitted inside a `context` or `adapter` body too, the same
way `service_decl`/`agent_decl` are admitted inside an `adapter` — the checker
reports `bynk.messages.outside_commons` precisely instead.

### message_entry {#rule-message_entry}

{{#grammar message_entry}}

One `"code" => "template"` entry inside a `messages` block. Both sides are
plain string literals — a template's `{name}` placeholders are resolved by a
compile-time string scan during lowering, not parsed as expressions.

### event_decl {#rule-event_decl}

{{#grammar event_decl}}

`event Name = { fields }` (Events track, spine #936) — a typed fact a context
may emit and other contexts' subscriber services may receive, via the
`Events` capability (`Events.emit[Name](...)`) and `from Events(Name)`.
Record body only. Context-only placement is a checker concern
(`bynk.event.outside_context`), not a grammar one — this rule is
syntactically permitted in a `commons`/`adapter` body too, the same way
`service_decl`/`agent_decl` are. An optional `@schema(N)` annotation (slice
3b) reuses `store_annotation`; `N`'s legality (a positive `Int` literal,
`@schema` alone, at most once) is a checker concern, not a grammar one.

**See also.** [Understand events](/book/guides/events/understand-events/).

### asset_decl {#rule-asset_decl}

{{#grammar asset_decl}}

`asset name = "path"` (#1883) — one static file the platform serves beside a
Worker, public by construction. `name` is a value of the built-in type `Asset`,
whose `.url` is the file's content-hashed URL. The path is a plain string
literal, relative to the project root, naming a file in its `assets/` folder.
An optional `at "/path"` serves it at that fixed URL path instead of a
content-hashed one (`at` is contextual, matched only here).
Placement (a context or commons, never an adapter) and the path rules (not
empty, not absolute, no `..` segment, inside `assets/`, the file exists) are
checker concerns, not grammar ones: this rule is admitted in every body, like
`event_decl`. `asset` is a contextual keyword, so it remains usable as an
ordinary identifier elsewhere.

### provider_decl {#rule-provider_decl}

{{#grammar provider_decl}}

A `provides` block implementing a capability, optionally `given` other
capabilities it depends on.

**Static semantics.**
{{#grammar-semantics provider_decl}}

**See also.** [Compose a provider from other capabilities](/book/guides/effects-and-capabilities/compose-a-provider/).

### provider_op {#rule-provider_op}

{{#grammar provider_op}}

One operation implementation in a provider: a capability operation with a body.

**Static semantics.**
{{#grammar-semantics provider_op}}

### given_clause {#rule-given_clause}

{{#grammar given_clause}}

Declares the capabilities a handler or provider may use.

**Static semantics.**
{{#grammar-semantics given_clause}}

## Actors (v0.45)

An `actor` is a nominal *boundary contract* — a closed, compiler-known
authentication scheme plus an optional sealed identity. A handler consumes one
on its `by` clause; the boundary verifies the scheme and mints the identity
before the body runs.

### actor_decl {#rule-actor_decl}

{{#grammar actor_decl}}

A boundary contract: `actor Name { auth = <Scheme> }`, optionally
`, identity = <Type>` (a context-ownable, sealed identity type). The **refinement
form** `actor Admin = Base where <predicate>` narrows a base actor by an
authorisation claim — e.g. `actor Admin = User where hasClaim("admin")`. The
predicate is a full expression (like a function contract), restricted by a
static-semantics rule to the closed **actor-claim catalogue** — `hasClaim(…)` /
`claimEquals(…, …)` composed with `&&` / `||` / `!` — over a `Bearer` base
(`bynk.actor.refinement_predicate_unsupported` / `…refinement_base_unsupported`).
This is one of the three `where` predicate tiers (see
[refinement](#rule-refinement)). Actors are context-only.

### scheme {#rule-scheme}

{{#grammar scheme}}

The closed authentication-scheme set: `None` (anonymous; identity `()`),
`Internal` (in-system/platform trust), `Bearer` (a JWT in `Authorization`, v0.47),
and `Signature` (an HMAC over the request body, for webhooks, v0.51). The
authenticated schemes carry a [`scheme_config`](#rule-scheme_config).

### scheme_config {#rule-scheme_config}

{{#grammar scheme_config}}

The keyed-args config an authenticated scheme carries — `Bearer(secret = "<ENV>")`
or `Signature(secret = "<ENV>", header = "<Header>", (timestamp = "<Header>",
tolerance = <seconds>)?)`. The checker validates which keys each scheme admits.

### scheme_arg {#rule-scheme_arg}

{{#grammar scheme_arg}}

One `key = value` pair in a [`scheme_config`](#rule-scheme_config); the value is a
string or integer literal (e.g. an integer `tolerance` in seconds).

### by_clause {#rule-by_clause}

{{#grammar by_clause}}

`by <binder>: <Actor>` on a handler, after the return type and alongside `given`
(v0.155 relocated it from before the parameter list). The verified actor binds to
`<binder>`; its identity is
`<binder>.identity`. The **binder is optional** (v0.50): `by <Actor>` verifies the
contract fail-closed but captures no identity (anonymous / verify-and-discard) —
the canonical form for an identity-less scheme like `Signature` (`by Webhook
(body: T)`). Omitting `by` entirely inherits the protocol's default actor — except
on HTTP, where `by` is required (`bynk.actor.missing_by_on_http`).

## Services & handlers

A `service` groups the handlers that respond to calls and external triggers.

### service_decl {#rule-service_decl}

{{#grammar service_decl}}

A service: a named group of handlers inside a context. The header may carry
service-level `by`/`given` **defaults** (v0.155), after the protocol clause — the
ambient contract every handler inherits unless it declares its own. A handler's
own `by` (or `given`) overrides the default outright; it does not merge.

**Example.** The `Visitor` default is stated once; the second route overrides it.
```bynk
context notes

service api from http by Visitor {
  on GET("/ping") () -> Effect[HttpResult[String]] {
    Ok("pong")
  }

  on GET("/notes/:id") (id: String) -> Effect[HttpResult[String]] {
    NotFound
  }
}
```

**Static semantics.**
{{#grammar-semantics service_decl}}

### service_protocol {#rule-service_protocol}

{{#grammar service_protocol}}

The `from <protocol>` header clause (v0.44): `from http`, `from cron`,
`from queue("<name>")`, `from websocket(in: I, out: O)` (v0.103, binding the
inbound/outbound frame types), or `from Events(E)` (Events track, spine #936),
optionally filtered by a structural [`event_pattern`](#rule-event_pattern)
(slice 1) and/or a [`schema_dispatch_clause`](#rule-schema_dispatch_clause)
(slice 4). Absent ⇒ a contract-mediated, `on call`-only service.

### event_pattern {#rule-event_pattern}

{{#grammar event_pattern}}

The structural delivery filter on a `from Events(E { field: value, .. })`
subscription header (Events track slice 1, spine #936): every emission of `E`
still reaches the fan-out mechanism (deliver-and-filter), and the
subscriber's own generated handler evaluates this as a runtime guard before
running its body — the handler's parameter is not narrowed to the matched
values. A trailing `..` is required whenever any field is listed;
`from Events(E)` (no braces) is the unfiltered form.

### event_pattern_field {#rule-event_pattern_field}

{{#grammar event_pattern_field}}

One `name: value` entry in an [`event_pattern`](#rule-event_pattern).

### event_pattern_value {#rule-event_pattern_value}

{{#grammar event_pattern_value}}

The value a pattern field is matched against: an `Int`/`String`/`Bool`
literal, or a nullary sum-type variant reference, bare (`Domestic`) or
qualified (`Region.Domestic`). A variant that carries a payload is rejected
by the checker — no nested sub-patterns in this slice.

### schema_dispatch_clause {#rule-schema_dispatch_clause}

{{#grammar schema_dispatch_clause}}

`via schema(N)` on a `from Events(...)` subscription header (Events track
slice 4, spine #936), after the closing `)` — dispatch by the envelope's
`schemaVersion` rather than the payload, parallel to
[`event_pattern`](#rule-event_pattern) but independent of it (a service may
carry either, both, or neither). Nested inside the `Events` protocol arm, so
`via` on `http`/`cron`/`queue`/`websocket` is a syntax error, not a checker
diagnostic. Delivery is deliver-and-filter, unchanged: every emission still
reaches every subscriber of the event type, and this is one more
independently-evaluated guard inside the subscriber's own generated
handler — sibling subscribers with the same or overlapping `N` are not
diagnosed as ambiguous. `N`'s legality (a positive `Int` literal) is a
checker concern (`bynk.event.bad_schema_dispatch`), not a grammar one.
Literal only in this slice; a range pattern (`via schema(2..)`) is a future
slice's unbuilt grammar.

**See also.** [Understand events](/book/guides/events/understand-events/).

### cors_policy {#rule-cors_policy}

{{#grammar cors_policy}}

The optional cross-origin (CORS) policy of a `from http` service (v0.131), in
header position before the handlers. `cors` is a contextual keyword (like
`store`/`key`), so it stays an ordinary identifier elsewhere.

**Example.**
```bynk
context api

service api from http {
  cors {
    origins: ["https://app.example.com"],
    credentials: false,
    maxAge: 1.hours,
  }

  on GET("/ping") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("pong")
  }
}
```

The checker validates the closed field set (`origins`/`headers`/`credentials`/
`maxAge`) and rejects `credentials: true` with the wildcard origin `["*"]`
([§5.7.1](/book/spec/static-semantics/#cors)). `Access-Control-Allow-Methods` is
derived from the routes, not declared.

### cors_field {#rule-cors_field}

{{#grammar cors_field}}

One `name: value` field inside a `cors { }` policy.

### security_policy {#rule-security_policy}

{{#grammar security_policy}}

The optional security-headers policy of a `from http` service (v0.141), in header
position beside `cors`. `security` is a contextual keyword, so it stays an
ordinary identifier elsewhere.

**Example.**
```bynk
context api

service api from http {
  security {
    hsts:    180.days,
    nosniff: true,
  }

  on GET("/ping") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("pong")
  }
}
```

`X-Content-Type-Options: nosniff` is stamped by default on every response;
`hsts` opts in to `Strict-Transport-Security`. The checker validates the closed
field set (`hsts`/`nosniff`) and requires the section be on a `from http` service
([§5.7.3](/book/spec/static-semantics/#security)).

### security_field {#rule-security_field}

{{#grammar security_field}}

One `name: value` field inside a `security { }` policy.

### limits_policy {#rule-limits_policy}

{{#grammar limits_policy}}

The optional request-limits policy of a `from http` service (v0.142), in header
position beside `cors` and `security`. `limits` is a contextual keyword, so it
stays an ordinary identifier elsewhere.

**Example.**
```bynk
context api

service api from http {
  limits {
    maxBody: 1_048_576,
  }

  on GET("/ping") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("pong")
  }
}
```

The checker validates the closed field set and requires the section be on a
`from http` service.

### limits_field {#rule-limits_field}

{{#grammar limits_field}}

One `name: value` field inside a `limits { }` policy.

### handler {#rule-handler}

{{#grammar handler}}

A handler: a call, HTTP, cron, queue, WebSocket (`on open`/`on close`, with
`on message` shared with the queue form), or Events (`on event`) entry point.

**Static semantics.**
{{#grammar-semantics handler}}

### call_handler {#rule-call_handler}

{{#grammar call_handler}}

`on call` — an in-process entry point, optionally named, callable across
contexts.

### http_handler {#rule-http_handler}

{{#grammar http_handler}}

`from http` — an HTTP route handler returning `Effect[HttpResult[T]]`.

**Static semantics.**
{{#grammar-semantics http_handler}}

**See also.** [HTTP](/book/reference/http/) · [Handle an HTTP request](/book/guides/entry-points/http/).

### http_method {#rule-http_method}

{{#grammar http_method}}

The HTTP methods a route may handle.

**Static semantics.**
{{#grammar-semantics http_method}}

### cron_handler {#rule-cron_handler}

{{#grammar cron_handler}}

`from cron` — a scheduled handler returning `Effect[Result[(), E]]`.

**Static semantics.**
{{#grammar-semantics cron_handler}}

**See also.** [Cron](/book/reference/cron/) · [Run a task on a schedule](/book/guides/entry-points/cron/).

### queue_handler {#rule-queue_handler}

{{#grammar queue_handler}}

`from queue` — a queue-message handler returning `Effect[Result[(), E]]`.

**Static semantics.**
{{#grammar-semantics queue_handler}}

**See also.** [Queue](/book/reference/queue/) · [Process a queued message](/book/guides/entry-points/queue/).

### ws_open_handler {#rule-ws_open_handler}

{{#grammar ws_open_handler}}

`from websocket` — the upgrade handshake (v0.103). Exactly one per service; it
names its actor with `by` and receives an owned `connection: Connection[out]` it
must dispose. The inbound-frame handler reuses the `on message` (queue) form.

### ws_close_handler {#rule-ws_close_handler}

{{#grammar ws_close_handler}}

`from websocket` — fires when the connection ends (v0.106); disposes the stored
connection.

**See also.** [WebSocket](/book/reference/websocket/) · [Handle a WebSocket connection](/book/guides/entry-points/websocket/).

### event_handler {#rule-event_handler}

{{#grammar event_handler}}

`from Events(E)` — a subscriber's one handler (Events track, spine #936),
`on event(e: E) -> Effect[()]`. Same generic shape as every other handler
form (params/`by`/`given`/body all uniform); the checker verifies the
parameter's type against the header's event type.

**See also.** [Understand events](/book/guides/events/understand-events/).

## Agents

An `agent` is a keyed, stateful entity: its state lives in `store` fields that
handlers read by name and write with `:=`.

### agent_decl {#rule-agent_decl}

{{#grammar agent_decl}}

An agent: a key, `store` fields, and handlers that read and write them (writes
commit atomically at handler end).

**Example.**
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
    let next = count + 1
    count := next
    next
  }
}
```

**Static semantics.**
{{#grammar-semantics agent_decl}}

**See also.** [Agents](/book/reference/agents/) · [Build a stateful agent](/book/guides/agents-and-state/stateful-agent/) · [The agent model](/book/guides/agents-and-state/the-agent-model/).

### key_decl {#rule-key_decl}

{{#grammar key_decl}}

The agent's identity: a key field whose value names an instance.

### store_field {#rule-store_field}

{{#grammar store_field}}

A `store` field (storage track): `store <name>: <Kind>[…] [@annotations] [= <init>]`
— an access-pattern slot of a declared storage kind. `store` is a contextual
keyword (also a valid identifier elsewhere). It is the agent's sole state surface
(ADR 0108); the legacy `state { }` block was removed at the parity slice.

> **`Cell`, `Map`, `Set`, `Cache`, and `Log` are functional.** A `Cell[T]` (v0.82)
> reads by bare name (implicit deref) and writes with `:=`; a `Map[K, V]` (v0.83,
> ADR 0110) is a storage map with effectful entry methods (`put`/`get`/`update`/
> `upsert`/`remove`/`contains`/`size`); a `Set[T]` (v0.84, ADR 0110) is a storage
> set with effectful membership methods (`add`/`remove`/`contains`/`size`); a
> `Cache[K, V]` (v0.87, ADR 0113) is a `Map` with per-entry TTL expiry, requiring
> `@ttl(<duration>)` and `given Clock` on its handlers (eviction reads the clock);
> a `Log[T]` (v0.95, ADR 0121) is an append-only, time-indexed sequence whose
> `append` stamps `Clock.now()` (`given Clock`) and whose reads are lazy `Query[T]`
> time-window builders (`since`/`before`/`between`/`recent`/`reversed`), with an
> optional `@retain(<duration>)`. All write ops are awaited with `<-` and commit
> atomically at handler end with the invariant gate (ADR 0109). The storage-kind
> **catalogue is closed at these five** — there is no `Queue` storage kind: a queue
> is a delivery concern reached through the `from queue` protocol, not agent state
> (ADR 0122).

### store_kind {#rule-store_kind}

{{#grammar store_kind}}

A storage kind applied to its element type(s): `Cell[Int]`, `Map[K, V]`. The head
is the kind name; the checker restricts it to the storage-kind catalogue.

### store_annotation {#rule-store_annotation}

{{#grammar store_annotation}}

A storage-field annotation (v0.85, ADR 0111): `@<name>` or `@<name>(<args>)`,
between the kind and the initialiser — `@ttl(5.minutes)`, `@indexed(by: orderId)`.
The name is matched against the closed registry (`@indexed`/`@ttl`/`@retain`/
`@bounded`); an unknown name, a wrong-kind use, or an annotation whose slice has
not landed is a checker diagnostic. v0.85 (slice 3a) lands the grammar and
registry; each annotation becomes functional with its kind's slice.

### annotation_arg {#rule-annotation_arg}

{{#grammar annotation_arg}}

One annotation argument: an optional `label:` then a value expression — `by: id`
(labelled, as in `@indexed`) or `5.minutes` (positional, as in `@ttl`). Arguments
are compile-time metadata, restricted to literals (and the `@indexed` field-name
labels) by the checker (ADR 0111 D4).

### invariant_decl {#rule-invariant_decl}

{{#grammar invariant_decl}}

An agent invariant: `invariant <name>: <predicate>`. A universally-quantified,
pure `Bool` predicate over the agent's `store` fields, runtime-checked at each
commit boundary. Invariants form a phase between the `store` fields and the
handlers.

**See also.** [Agent invariants](/book/reference/agent-invariants/).

### transition_decl {#rule-transition_decl}

{{#grammar transition_decl}}

An agent step invariant: `transition <name>: <predicate over old/new>`. A pure
`Bool` predicate over the `old`/`new` committed-state pair, runtime-checked at the
commit boundary (from the second commit onward). Transitions sit in the same phase
as invariants, between the `store` fields and the handlers.

**See also.** [Step invariants](/book/reference/agent-invariants/#step-invariants).

## Expressions

Bynk is expression-oriented: a block's value is its final expression. Operators
follow the usual precedence (see [Operators & built-ins](/book/reference/operators/)).

### expression {#rule-_expression}

{{#grammar _expression}}

Any expression: control flow, refinement checks, operators, or a primary.

### primary {#rule-_primary}

{{#grammar _primary}}

The atomic and postfix expressions: literals, names, calls, field and method
access, constructors, and parenthesised expressions.

### if_expr {#rule-if_expr}

{{#grammar if_expr}}

A conditional expression; both branches must have the same type.

**Static semantics.**
{{#grammar-semantics if_expr}}

### match_expr {#rule-match_expr}

{{#grammar match_expr}}

Pattern-matches a value against variants; must be exhaustive.

**Example.**
```bynk,fragment
match s {
  Pending => "awaiting shipment"
  Shipped(tracking: t) => t
  Cancelled(reason: r) => r
}
```

**Static semantics.**
{{#grammar-semantics match_expr}}

**See also.** [Pattern-match with `match`](/book/guides/type-system/match/).

### is_expr {#rule-is_expr}

{{#grammar is_expr}}

A refinement/variant check that also narrows the value's type in the `true`
branch. A variant pattern's **nested payload patterns are structural tests too**:
`r is Rejected(RefinementViolation(_))` tests the outer tag *and* the inner one,
the same tests a `match` arm applies. A payload bound to a plain name (`is Ok(x)`)
or a wildcard (`is Ok(_)`) adds no nested test — it only narrows.

**Static semantics.**
{{#grammar-semantics is_expr}}

**See also.** [Narrow and bind with `is`](/book/guides/type-system/narrow-with-is/).

### binary_expr {#rule-binary_expr}

{{#grammar binary_expr}}

The binary operators, in precedence order from `||` to `*`/`/`.

**Static semantics.**
{{#grammar-semantics binary_expr}}

**See also.** [Operators & built-ins](/book/reference/operators/).

### unary_expr {#rule-unary_expr}

{{#grammar unary_expr}}

Logical negation `!` and numeric negation `-`.

### method_call {#rule-method_call}

{{#grammar method_call}}

Calls a method on a value: `receiver.method(args)`.

**Static semantics.**
{{#grammar-semantics method_call}}

### field_access {#rule-field_access}

{{#grammar field_access}}

Reads a field of a record or agent state: `value.field`.

**Static semantics.**
{{#grammar-semantics field_access}}

### call {#rule-call}

{{#grammar call}}

Calls a function or constructs a variant: `name(args)`.

**Static semantics.**
{{#grammar-semantics call}}

### record_construction {#rule-record_construction}

{{#grammar record_construction}}

Builds a record value: `Type { field: value, … }`.

**Static semantics.**
{{#grammar-semantics record_construction}}

### field_init {#rule-field_init}

{{#grammar field_init}}

One field in a record construction: `name: value`, or shorthand `name`.

### record_spread {#rule-record_spread}

{{#grammar record_spread}}

Builds a record from an existing one, overriding some fields: `{ ...base, field:
value }`.

**Static semantics.**
{{#grammar-semantics record_spread}}

### question_expr {#rule-question_expr}

{{#grammar question_expr}}

The `?` operator: unwraps a `Result`, propagating the error on failure.

**Static semantics.**
{{#grammar-semantics question_expr}}

**See also.** [Work with `Result` and optional values](/book/guides/type-system/result-and-optionals/).

### ok_expr {#rule-ok_expr}

{{#grammar ok_expr}}

The `Ok` constructor of `Result` (or `HttpResult`).

**Static semantics.**
{{#grammar-semantics ok_expr}}

### err_expr {#rule-err_expr}

{{#grammar err_expr}}

The `Err` constructor of `Result`.

**Static semantics.**
{{#grammar-semantics err_expr}}

### some_expr {#rule-some_expr}

{{#grammar some_expr}}

The `Some` constructor of `Option`.

**Static semantics.**
{{#grammar-semantics some_expr}}

### none_expr {#rule-none_expr}

{{#grammar none_expr}}

The `None` constructor of `Option`.

**Static semantics.**
{{#grammar-semantics none_expr}}

### effect_pure_expr {#rule-effect_pure_expr}

{{#grammar effect_pure_expr}}

`Effect.pure(x)` — lifts a pure value into an `Effect`.

### val_expr {#rule-val_expr}

{{#grammar val_expr}}

`Val[T]` — fabricates a valid inhabitant of type `T` (drawn from its refinement
domain), optionally pinned to a specific value with `Val[T](v)`. Valid only in
test bodies. Replaces the retired `Mock[T]` (v0.114).

**Static semantics.**
{{#grammar-semantics val_expr}}

**See also.** [Write tests](/book/guides/testing/write-tests/).

### val_arg {#rule-val_arg}

{{#grammar val_arg}}

The pin arguments to a `Val[T]`: positional values or a record of field pins.

### wire_expr {#rule-wire_expr}

{{#grammar wire_expr}}

`Wire(<String>)` — a raw, pre-validation argument to a `system`-tier service
address (testing-the-boundary). The inner `String` is the wire form the boundary
receives *unvalidated* — a body's JSON text or a path segment — so a `case` can
drive the router with input the type system forbids and observe the rejection.
Valid only as a service-address argument in a `system`-tier `case`; the router
validates it, so no refined value is ever minted from a `Wire`.

### lambda_expr {#rule-lambda_expr}

{{#grammar lambda_expr}}

A lambda (v0.20a): `(o) => o.paid`, `(acc, t) => acc + t`, `() => 0`, or with a
block body `(o) => { … }`. Always parenthesised; `=>` is the **value** arrow
(shared with `match` arms), `->` stays the type arrow. Parameter annotations
are optional where an expected function type supplies them — and required
otherwise. A lambda may close over and call a `given` capability; its
effectfulness is read off its body (an effect operation makes it effectful,
wrapping the result in `Effect`).

**Static semantics.**
{{#grammar-semantics lambda_expr}}

### lambda_param {#rule-lambda_param}

{{#grammar lambda_param}}

One lambda parameter, with an optional type annotation.

**Static semantics.**
{{#grammar-semantics lambda_param}}

### list_literal {#rule-list_literal}

{{#grammar list_literal}}

A `List` literal (v0.20b): `[1, 2, 3]`, with an optional trailing comma. A
*leading* `[` only — type application (`name[T](…)`) stays a postfix form on
a callee identifier, and its `[` must sit on the same line as the callee.
Elements check against the expected element type when one is supplied; an
empty `[]` needs an expected type to infer its element type from.

**Static semantics.**
{{#grammar-semantics list_literal}}

### paren_expr {#rule-paren_expr}

{{#grammar paren_expr}}

A parenthesised expression, for grouping.

### self_expr {#rule-self_expr}

{{#grammar self_expr}}

`self` — the receiver inside a method or agent handler.

**Static semantics.**
{{#grammar-semantics self_expr}}

## Patterns & matching

The patterns used in `match` arms and `is` checks.

### match_arm {#rule-match_arm}

{{#grammar match_arm}}

One arm of a `match`: a pattern, an optional `if` guard over the pattern's
bindings, `=>`, and a result expression. A guarded arm never satisfies
exhaustiveness (its guard may fail at runtime). `refined_pattern` (#472) is
admitted only here — a match arm's top-level pattern — not through `pattern`
generally, so it is never reachable from `is` or from a nested payload
position; a match arm's pattern is always followed by a fixed terminator
(`if`/`=>`), unlike an expression position, where `refinement`'s own
`&&`-joined predicate list would be ambiguous with the surrounding grammar.

**Static semantics.**
{{#grammar-semantics match_arm}}

**See also.** [Pattern-match with `match`](/book/guides/type-system/match/).

### pattern {#rule-_pattern}

{{#grammar _pattern}}

A pattern: a wildcard, a literal, a binding, a variant pattern, an
or-pattern, or a parenthesized pattern. A lowercase-led identifier is a
binding (it matches anything and binds the value); an uppercase-led one is a
nullary variant — in the concrete grammar both parse as `variant_pattern`.

### variant_pattern {#rule-variant_pattern}

{{#grammar variant_pattern}}

Matches a sum-type variant, optionally binding its payload fields. The
payload list's `(` must sit on the same line as the variant name, as a call's
does, in every pattern position: a `(` that opens a new line is not part of
the pattern. So `expect r is None` above a `()` line is a nullary pattern and
a unit, not `None()`, and a match arm's `Settled` above `(amount) =>` is a
parse error.

**Static semantics.**
{{#grammar-semantics variant_pattern}}

### wildcard_pattern {#rule-wildcard_pattern}

{{#grammar wildcard_pattern}}

`_` — matches anything, binding nothing.

### literal_pattern {#rule-literal_pattern}

{{#grammar literal_pattern}}

Matches a primitive scrutinee (`Int`/`String`/`Bool`, or a refinement over one)
by value equality — an integer (optionally negated), a string, or a boolean.
A literal-pattern `match` needs a wildcard `_` arm to be exhaustive, except over
`Bool`, which is complete once both `true` and `false` appear.

### or_pattern {#rule-or_pattern}

{{#grammar or_pattern}}

`p₁ | p₂` — matches if either alternative matches, left-associative
(`p₁ | p₂ | p₃` is `(p₁ | p₂) | p₃`). Every alternative must bind the same
set of names, a name shared across alternatives must have the same type
(including refinement) in each, and every alternative must match the same
value type. `|` is a pattern-position operator only, distinct from boolean
`||`.

**Static semantics.**
{{#grammar-semantics or_pattern}}

**See also.** [Pattern-match with `match`](/book/guides/type-system/match/).

### paren_pattern {#rule-paren_pattern}

{{#grammar paren_pattern}}

Parentheses around a pattern — transparent grouping, most useful around an
or-pattern for readability (`is (Held(...) | Confirmed(...))`). Optional:
`is` already parses one whole pattern, `|`-chain included, without them.
Never admits a `refined_pattern` inside (#472) — see below.

### refined_pattern {#rule-refined_pattern}

{{#grammar refined_pattern}}

`p 'where' predicate` (#472) — a runtime guard on a pattern, reusing the
closed refinement-predicate catalogue a `type X = Base where P` declaration
uses ([`refinement`](#rule-refinement)). v1 admits only `_ where predicate` —
the compiler rejects any other inner form (`bynk.parse.refined_pattern_inner`).
Admitted only against a literal-kind scrutinee (`Int`/`String`); a guard, not
a narrowing — matching a refined arm does not change any static type. Like an
`if` guard, a refined arm alone is never exhaustive, and it is `match`-only —
one on the right of `is` is rejected (`bynk.types.is_refined_pattern`).
Composes with an or-pattern only as the whole thing's outer wrapper —
`(p₁ | p₂) where predicate` refines the alternation as a unit — never as one
alternative among others (`match_arm`'s pattern field is a single
`choice($._pattern, $.refined_pattern)`, not a repetition, so a refined
pattern can never appear as one `|`-separated alternative alongside others).

**See also.** [Pattern-match with `match`](/book/guides/type-system/match/).

### pattern_binding {#rule-_pattern_binding}

{{#grammar _pattern_binding}}

A binding within a variant pattern: a named binding, or a full sub-pattern
matched against the payload field. A lowercase-led identifier binds the field, an
uppercase-led one discriminates a nested nullary variant (`Err(PollClosed)`), `_`
ignores it, and a nested variant recurses (`Some(Ok(x))`).

### named_binding {#rule-named_binding}

{{#grammar named_binding}}

Binds a payload field by name, matching it against a sub-pattern: `field: name`
(or `field: _` to ignore).

## Statements

A block is a sequence of statements ending in an optional value expression.

### block {#rule-block}

{{#grammar block}}

A braced sequence of statements with an optional trailing expression, which is
the block's value.

### statement {#rule-_statement}

{{#grammar _statement}}

A statement: a `let`, an effectful `let`, a `let … else`, a `:=` store write, an
async send, or an assertion.

### let_stmt {#rule-let_stmt}

{{#grammar let_stmt}}

Binds a pure value: `let name = expr`.

**Static semantics.**
{{#grammar-semantics let_stmt}}

### effect_let_stmt {#rule-effect_let_stmt}

{{#grammar effect_let_stmt}}

Binds the result of an effect: `let name <- effect`. In a test `case` body it may
carry a trailing [`call_site_actor`](#rule-call_site_actor) clause naming the
principal the case acts as when the effect drives a service handler.

**Static semantics.**
{{#grammar-semantics effect_let_stmt}}

### call_site_actor {#rule-call_site_actor}

{{#grammar call_site_actor}}

The test-body `by <Actor>(<identity>)` clause (v0.182): names the actor a `case`
acts as when it drives a service handler, and supplies the identity value. Written
`by User("bob")` for an identity-carrying actor, or `by Visitor` (no argument) for
a unit-identity actor. It trails an effect-let's value, or sits between a fault
claim's call and `faults` ([`faults_expr`](#rule-faults_expr), #1812). Distinct
from [`by_clause`](#rule-by_clause), the handler
form, which binds an actor and admits a sum but carries no identity argument.

### let_else_stmt {#rule-let_else_stmt}

{{#grammar let_else_stmt}}

Matches a value against a pattern, binding its names for the rest of the block,
with a fallback for the values it does not match: `let Ok(x) = r else { … }`, or
`let Ok(x) <- effect else { … }` for an effect's result (#2010). The `else` is
what the enclosing block produces instead of its remaining statements, so it has
that block's type. It is a [`block`](#rule-block) or a
[`let_else_match`](#rule-let_else_match).

**Static semantics.**
{{#grammar-semantics let_else_stmt}}

### let_else_match {#rule-let_else_match}

{{#grammar let_else_match}}

The `else match { arms }` form of a `let … else`: [`match_arm`](#rule-match_arm)s
over the values the `let` pattern does not cover, so an arm can bind them
(`Err(why) => …`).

### let_pattern {#rule-_let_pattern}

{{#grammar _let_pattern}}

The patterns that may follow `let` in a `let … else`: a literal, a parenthesised
pattern, or a variant written with a payload or a qualifier. A bare name after
`let` is always a binding, so a nullary variant is written `Status.Done` or
`(Done)`.

### let_variant_pattern {#rule-_let_variant_pattern}

{{#grammar _let_variant_pattern}}

A [`variant_pattern`](#rule-variant_pattern) that cannot be mistaken for a
binding name: qualified (`Status.Done`, `Status.Shipped(at)`) or with a payload
(`Ok(x)`, `Shipped(at)`).

### let_or_pattern {#rule-_let_or_pattern}

{{#grammar _let_or_pattern}}

An [`or_pattern`](#rule-or_pattern) after `let`, whose first alternative opens as
a [`let_pattern`](#rule-_let_pattern): `let Ok(x) | Err(x) = r else { … }`.

### effect_send_stmt {#rule-effect_send_stmt}

{{#grammar effect_send_stmt}}

Sends an effect asynchronously without awaiting its reply: `~> effect`. The
caller does not wait and binds nothing; legal only when the reply is `Effect[()]`
(see the error gate below). Contrast `let _ <- effect`, which awaits the reply
and discards it.

**Static semantics.**
{{#grammar-semantics effect_send_stmt}}

### do_stmt {#rule-do_stmt}

{{#grammar do_stmt}}

Performs a unit effect as a statement: `do effect` (v0.146, ADR 0170). The
binder-free spelling of `let _ <- effect` when the awaited value is `()` — the
effect runs and joins the handler, its unit result discarded. Legal only in an
effectful body; the operand must be `Effect[()]`. To await and discard a
*valued* reply, keep `let _ <- effect`, so throwing away a real value stays
visible.

**Static semantics.**
{{#grammar-semantics do_stmt}}

### assign_stmt {#rule-assign_stmt}

{{#grammar assign_stmt}}

`name := expr` (v0.81, storage track) — a `Cell` store write. The unconditional
write form; `.update(fn)` is the read-modify-write form. ADR 0108.

### expect_expr {#rule-expect_expr}

{{#grammar expect_expr}}

`expect` — checks a `Bool` predicate in a `case`, observes a capability seam
([`observation_expr`](#rule-observation_expr)), or claims a fault
([`faults_expr`](#rule-faults_expr)).

**Static semantics.**
{{#grammar-semantics expect_expr}}

### observation_expr {#rule-observation_expr}

{{#grammar observation_expr}}

An observation over a capability seam, inside a `case`: a `Cap.op` subject (named,
not called) and a matcher — `called` (optionally `once` / `<n> times`, optionally
`with <predicate>`), `never called`, or `before Cap.op`. Calls are recorded
automatically at the seam in the test build, so no setup is needed. The matcher
words are contextual keywords.

**See also.** [Observation](/book/reference/testing/#observation).

### faults_expr {#rule-faults_expr}

{{#grammar faults_expr}}

A fault claim, inside a `case`: `expect <call> faults` holds when awaiting the
call throws (an injected `stub … fails`, a real provider's failure, an invariant
violation) rather than returns. The subject is the call itself, not a value bound
from it. An optional [`call_site_actor`](#rule-call_site_actor) between the call
and `faults` supplies the caller identity the addressed handler reads (#1812).
`faults` is a contextual keyword.

**See also.** [Claiming a fault](/book/reference/testing/#faults).

### trace_expr {#rule-trace_expr}

{{#grammar trace_expr}}

`trace(Cap.op)` — the escape hatch: the recorded calls of a capability operation as
a `List` of per-operation records, in call order, asserted with the ordinary `List`
surface. A test-only builtin.

**See also.** [Observation](/book/reference/testing/#observation).

### binding_name {#rule-_binding_name}

{{#grammar _binding_name}}

The name bound by a `let`: an identifier, or `_` to discard.

## Testing constructs

Cases, `stub` and `env` clauses. See also the top-level
[`suite_decl`](#rule-suite_decl).

### case {#rule-case}

{{#grammar case}}

A single named `case`, typically ending in `expect`s. An optional `as <tier>`
classifier (`unit`, `integration`, or `system`) records the test level, and the
body may open with `stub` and `env` clauses before its statements.

**Example.**
```bynk,fragment
case "a fresh counter starts at zero" {
  let n <- Counter(CounterId.unsafe("fresh")).current()
  expect n == 0
}
```

**Static semantics.**
{{#grammar-semantics case}}

**See also.** [Testing](/book/reference/testing/) · [Write tests and stub collaborators](/book/guides/testing/write-tests/).

### property_decl {#rule-property_decl}

{{#grammar property_decl}}

A generative `property` (v0.114) — the generative sibling of `case`. Its body is
a single [`for all`](#rule-for_all) binder; the runner produces the subjects.

**Example.**
```bynk,fragment
property "more discount, never a higher price" {
  for all p: Price, a: Percent, b: Percent where a <= b {
    expect discount(p, b) <= discount(p, a)
  }
}
```

**Static semantics.**
{{#grammar-semantics property_decl}}

**See also.** [Testing](/book/reference/testing/) · [Write tests](/book/guides/testing/write-tests/).

### for_all {#rule-for_all}

{{#grammar for_all}}

The `for all` binder: one or more [bindings](#rule-for_all_binding) over
generated inhabitants, an optional `where` filter (a pure `Bool` applied before
the body runs), and a predicate body of `expect`s.

**Static semantics.**
{{#grammar-semantics for_all}}

### for_all_binding {#rule-for_all_binding}

{{#grammar for_all_binding}}

A single `for all` binding, `x: T` — binds `x` to a generated inhabitant of the
refinement-generable type `T`.

### stub_clause {#rule-stub_clause}

{{#grammar stub_clause}}

A test-scope stub for one capability operation — `stub Cap.op(<pattern>, …)
<rhs>`. Each argument pattern is `_` (any) or a value expression; the right-hand
side is `returns <expr>`, `returns each [ <outcome>, … ]` (a scripted sequence of
outcomes, each `fails` or an expression), or `fails`. Its own keyword since the
keyword-hygiene batch (#548) — no longer punned on the production
[`provider_decl`](#rule-provider_decl) (`provides Cap = Impl …`); it appears as a
`suite` item or as a leading item in a `case` body. `returns` / `each` / `fails`
are contextual words.

**See also.** [Write tests and stub collaborators](/book/guides/testing/write-tests/).

### env_clause {#rule-env_clause}

{{#grammar env_clause}}

The variables and secrets a suite's participants see (#2134): a `suite` item, or
a leading item in a `case` body, whose entries layer over the suite's name by
name. At `system` the entries go on each participant's Worker `env`; at `unit`
and `integration` they are what the `Secrets` double answers. `env` is a
contextual word.

**Example.**
```bynk,fragment
env {
  PAYMENTS_PROCESSOR = "fake"
  AUTH_JWT_SECRET = "test-secret"
}
```

**Static semantics.**
{{#grammar-semantics env_clause}}

**See also.** [Configuring a suite](/book/reference/testing/#env).

### env_entry {#rule-env_entry}

{{#grammar env_entry}}

One `NAME = "value"` entry of an [`env` clause](#rule-env_clause). The value is
a `String` literal.
