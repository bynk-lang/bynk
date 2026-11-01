---
title: "§6 The type system"
---
This chapter defines Bynk's types: what each kind is, how it is written, what its
values are, how a value is constructed, and what identity the type has. It is the
definitional layer. The rules a well-formed program must satisfy — type
compatibility, admission, exhaustiveness, and the rest — are stated in
[§5](/book/spec/static-semantics/), which references this chapter.

## §6.1 Type kinds

A `type` declaration ([§4.2.1](/book/spec/syntactic-grammar/#421-type_decl)) introduces a
named type in one of six kinds.

### §6.1.1 Base types

{{#grammar base_type}}

The primitive types are `Int`, `String`, `Bool`, and `Float` (v0.21). Their
values are, respectively, integer literals, string literals, the two booleans
`true` and `false`, and float literals
([§3.2.1a](/book/spec/lexical-grammar/#321a-float_literal)). The **unit type** is
written `()` and has the single value `()`. Base types are the only types that
may be refined or made opaque.

`Int` and `Float` are **distinct and incompatible**: there is no implicit
numeric coercion in any direction, and mixing them in an operation is
`bynk.types.no_numeric_coercion`
([§5.2](/book/spec/static-semantics/#52-well-typedness)). Conversion is explicit, via
the numeric kernel (also §5.2). Both erase to the same TypeScript `number`
([§7.3.1](/book/spec/emission/#731-types)) — the distinction is enforced entirely by
the checker.

### §6.1.2 Refined types

A refined type is a base type narrowed by a `where` refinement
([§4.2.4](/book/spec/syntactic-grammar/#424-refined_type)). Its values are exactly those
values of the base type that satisfy every predicate of the refinement (predicate
meanings: [§6.3](#63-refinement-predicates)). A refined type is a **distinct
named type**: it does not interchange with its base or with another refined type
over the same base. Values enter it by admission ([§6.4](#64-admission--construction)).

A `type` alias with no `where` clause — for example `type Id = Int` — is the
degenerate refined type: a distinct named type over its base, carrying the same
`.of` constructor and compile-time literal admission, but admitting every value
of the base.

### §6.1.3 Opaque types

{{#grammar opaque_type}}

An opaque type has **nominal identity over a base** type whose representation is
hidden outside the type's defining module. Its values are constructed only
through its API — `.of`, or `.unsafe` within the defining `commons`
([§6.4](#64-admission--construction)) — never with record syntax. The base value
is recoverable only through `.raw`, and only within the defining `commons`. An
opaque type MAY also carry a `where` refinement. Opaque types are **excluded**
from literal admission. An opaque type is not `Ord` outside its defining unit
unless its declaration says `derives Ord` (§5.2).

### §6.1.4 Sum types

A sum type ([§4.2.7](/book/spec/syntactic-grammar/#427-sum_type)) is a tagged union of one
or more variants; a variant MAY carry a named payload. A value is constructed by
naming a variant (with its payload arguments, if any) and is consumed by `match`
([§4.6.4](/book/spec/syntactic-grammar/#464-match_expr)) or `is`
([§4.6.5](/book/spec/syntactic-grammar/#465-is_expr)). A sum type is represented as a
discriminated union over a tag.

A sum type MAY be **recursive** (#2613): a variant's payload may have the sum's
own type, directly, through another sum or record, or inside an `Option`,
`List` or `Map`, and declarations may recurse mutually:

```bynk
commons trees {
  type Tree =
    | Leaf
    | Node(left: Tree, value: Int, right: Tree)
}
```

A recursive sum with no variant that ends the recursion
(`type Loop = | Again(next: Loop)`) is accepted, but it has no values: it can
never be constructed.

### §6.1.5 Record types

A record type ([§4.2.5](/book/spec/syntactic-grammar/#425-record_type)) is a product of
named, immutable fields. A value is constructed by giving every field
([§4.6.11](/book/spec/syntactic-grammar/#4611-record_construction)), read by field access
([§4.6.9](/book/spec/syntactic-grammar/#469-field_access)), and updated by the spread form
([§4.6.13](/book/spec/syntactic-grammar/#4613-record_spread)), which copies and overrides.
A record MUST NOT contain itself through record fields alone: a field whose
type is the record, or a chain of fields through other records that leads back
to it (`type A = { b: B }` with `type B = { a: A }`), is
`bynk.resolve.recursive_record_field`, reported on each field in the cycle. A
value of such a type could never be built, and the compiler could not
synthesise the default value it needs for one (for a `store` field, say). A
cycle that passes through an `Option`, `List` or `Map`, or through a sum type,
is permitted, because that position can hold a value that ends the chain
(#2613):

```bynk
commons files {
  type Dir = { name: String, children: List[Dir] }
  type Chain = { value: Int, next: Option[Chain] }
}
```

A record, sum or enum declaration MAY end with `derives Ord` ([§4.2.1](/book/spec/syntactic-grammar/#421-type_decl)),
which makes the type `Ord`: its values are ordered by field, or by variant, in
declaration order ([§5.2](/book/spec/static-semantics/#52-well-typedness)).

### §6.1.6 Enum types

An enum ([§4.2.10](/book/spec/syntactic-grammar/#4210-enum_type)) is sugar for a sum type
all of whose variants carry no payload. Its values are its constant names.

## §6.2 Built-in generic types

Nine generic types are built in. Their runtime shapes are normatively the
[runtime-library contract (§7.4)](/book/spec/runtime-library/); this section defines their
surface.

- **`Result[T, E]`** — either `Ok(T)` (success) or `Err(E)` (error). Errors are
  values; a fallible computation returns a `Result` rather than throwing.
- **`Option[T]`** — either `Some(T)` (a value) or `None` (absence).
- **`Effect[T]`** — an effectful computation yielding a `T`. It has no surface
  constructor other than `Effect.pure(x)`, which lifts a pure `x`; an `Effect` is
  sequenced with the `<-` bind ([§4.8.4](/book/spec/syntactic-grammar/#484-effect_let_stmt)).
  Effects are how a program reaches the outside world ([§5.5](/book/spec/static-semantics/#55-effects-capabilities--providers)).
- **`HttpResult[T]`** — an HTTP response, the return shape of HTTP handlers
  (see [§5.7](/book/spec/static-semantics/#57-handlers)).
- **`List[T]`** (v0.20b) — an **immutable** ordered sequence, constructed by
  the list literal `[a, b, c]` or `List.empty()`; every operation returns a
  new list, none mutates. Kernel operations and the eager method vocabulary
  (`map`/`filter`/`flatMap`/`sortBy`/… builders, `count`/`sum`/`first`/…
  terminals; v0.88, ADR 0116): [§5.10](/book/spec/static-semantics/#510-collections).
  The predecessor `bynk.list` free functions whose method forms now exist are
  **deprecated** (`bynk.list.deprecated_function`, v0.91) — they still work.
- **`Map[K, V]`** (v0.20b) — an **immutable**, insertion-ordered key→value
  map, constructed by `Map.empty()` and grown with `insert`; updating an
  existing key keeps its position. The key type is confined to
  **value-keyable** types: `String`, `Int`, or a refined/opaque type over them
  (`bynk.types.unkeyable_map_key` otherwise). A type parameter is admitted in
  key position — it can only ever be instantiated through a concrete
  `Map[K, V]` reference elsewhere, and that site is checked.
- **`Stream[T]`** (v0.100) — a **lazy, pull-shaped sequence of values produced
  over time**, distinct from `Effect[T]` (resolves once) and `Query[T]` (a
  snapshot read). Built with `Stream.of(xs)`, transformed lazily (`map`/`take`),
  and drained with the terminal `collect() -> Effect[List[T]]`. It is
  **non-boundary** (§6.5): a live source is built and consumed in place, never
  persisted nor sent across a boundary (`bynk.types.stream_at_boundary`), and not
  value-comparable (`bynk.types.stream_not_comparable`).
- **`Query[T]`** (v0.92) — a **lazy read over a `store`'s storage**, carrying the
  same combinator vocabulary as the eager `List` methods but dispatched by
  receiver provenance; builders are pure and terminals are `Effect`-typed
  ([§5.10](/book/spec/static-semantics/#510-collections)). It is first-class by reference
  but **non-boundary** (§6.5): `bynk.types.query_at_boundary`.
- **`Connection[F]`** (v0.102) — a **held resource**: a typed handle to a
  long-lived WebSocket connection whose server-sent frame type is `F`, the one
  concrete instance of the closed **`Held`** kind. Held values are
  **runtime-produced** (no constructor — they arrive from a capability operation
  or a handler parameter the framework supplies) and governed by an **ownership
  (linearity) discipline** ([§5.4.2](/book/spec/static-semantics/#542-held-resource-linearity-v0102)).
  They are **non-boundary** (§6.5, `bynk.types.held_at_boundary`) and not
  value-comparable (`bynk.types.held_not_comparable`).

`ValidationError` ([§4.2.19](/book/spec/syntactic-grammar/#4219-validation_error_type)) is
the error type produced by a refined or opaque `.of` constructor when validation
fails.

`JsonError` (v0.22b) is the error type produced by `Json.decode`
([§5.2](/book/spec/static-semantics/#52-well-typedness)) — a compiler-known **record**
with `String` fields `kind`, `path`, and `message`, inspectable by ordinary
field access. Like `ValidationError` it is a built-in name, not a declarable
shape; neither error builtin passes through the JSON codec itself
(`bynk.types.json_uncodable`).

> [!NOTE]
> The `Ok` and `Err` constructors are shared between `Result` and `HttpResult`;
> where the target is ambiguous a program must qualify the constructor
> ([§5.2](/book/spec/static-semantics/#52-well-typedness)). This note is informative.

## §6.3 Refinement predicates

A refinement ([§4.2.11](/book/spec/syntactic-grammar/#4211-refinement)) is one or more
built-in predicates joined by `&&`. Each predicate applies to a specific base
([§5.3](/book/spec/static-semantics/#53-refinement--admission)).

{{#grammar predicate_name}}

**On `Int` and `Float`:**

| Predicate | Holds when |
|---|---|
| `NonNegative` | value ≥ 0 |
| `Positive` | value > 0 |
| `InRange(lo, hi)` | lo ≤ value ≤ hi (inclusive) |

`InRange` bounds are numeric literals whose type must **match the base**
(v0.21): integer bounds on `Int` (`InRange(0, 10)`), float bounds on `Float`
(`InRange(0.0, 1.0)`). A bound of the other numeric type — or a mix — is
`bynk.types.no_numeric_coercion`. On `Float`, `Positive` excludes `0.0`
exactly as it excludes `0` on `Int`, and the `.of` constructor additionally
requires the value to be **finite** ([§7.2](/book/spec/emission/#72-targets)).

```bynk
commons pricing {
  type Price = Float where Positive
  type Ratio = Float where InRange(0.0, 1.0)

  fn discounted(p: Price, r: Ratio) -> Float {
    p * r
  }

  fn toCents(f: Float) -> Int {
    (f * 100.0).round()
  }
}
```

**On `String`:**

| Predicate | Holds when |
|---|---|
| `NonEmpty` | length ≥ 1 |
| `MinLength(n)` | length ≥ n |
| `MaxLength(n)` | length ≤ n |
| `Length(n)` | length = n |
| `Matches(regex)` | the whole string matches `regex`, anchored |

## §6.4 Admission & construction

A value enters a refined or opaque type through **checked construction** — the
`.of` constructor — or, for a compile-time constant, through **literal
admission**. Opaque types additionally have an unchecked escape hatch, `.unsafe`,
confined to their defining `commons`; a refined type has none.

- **`.of(v) -> Result[T, ValidationError]`** — checked construction, on both
  refined and opaque types. The predicates are tested **at run time**; the result
  is `Ok(T)` or `Err(ValidationError)`. This is the path for values not known
  until run time — input, parameters, computed values.
- **Literal admission** — a compile-time literal written where a **refined** type
  is expected is checked **at compile time** and admitted directly, with no
  `Result`. The admissible positions, and the failure when a literal violates the
  predicate, are specified in
  [§5.3](/book/spec/static-semantics/#53-refinement--admission). Opaque types are
  **excluded** from literal admission.
- **`.unsafe(v) -> T`** — unchecked construction, available on **opaque types
  only** and **only within the defining `commons`**
  (`bynk.types.opaque_unsafe_outside`). The value is taken to satisfy the type
  without a check — the deliberate escape hatch by which an opaque type's own
  module mints a value from the representation it alone can see. A **refined**
  type has no `.unsafe`: its representation *is* its base, always constructible
  through the validating `.of`, so every refined value in a checked program has
  had its predicate verified — at run time or compile time — with no unchecked
  third door.

The flow-sensitive counterpart to `.of` is the `is` narrowing
([§4.6.5](/book/spec/syntactic-grammar/#465-is_expr)): in a guard, an identifier of the
base type is narrowed to the refined type without constructing a `Result`.

## §6.4a Function types (v0.20a)

`A -> B` is a type form: a value of it is a lambda, a named function used as
a value, or a function-typed parameter/local. It is **effectful** exactly
when `B` is `Effect[_]` — the same structural rule that classifies function
declarations, so the pure-vs-effectful (`map`-vs-`traverse`) distinction
needs no new machinery. Compatibility is **contravariant in parameters and
covariant in the return type**: `t` is usable where `u` is expected when each
of `u`'s parameter types is usable where `t`'s is, and `t`'s return is usable
where `u`'s is. This is the sound generalisation of Bynk's only subtyping
(refined → base widening): the covariant-everywhere alternative would let
unvalidated base values flow into a refined-typed function body. **Function
types are admissible only in non-boundary positions** — fn/lambda parameters,
returns, and locals; everywhere a value would serialise, persist, or cross a
boundary they are rejected ([§5.8](/book/spec/static-semantics/#58-boundaries--cross-context),
`bynk.types.function_at_boundary`). A generic function's type parameters are
type variables, instantiated per call site and erased at emission — unconstrained
unless they declare an `Eq` or `Ord` bound (§5.2); they never appear in a checked program's expression types outside
the generic function's own body.

## §6.5 Type compatibility & boundaries

**Within a context, type identity is nominal.** Two named types — refined,
opaque, or alias — are distinct even over the same base; an opaque type is
distinct from and hides its base. A value of one named type is not
interchangeable with another, nor with the bare base.

**Across a context boundary, compatibility is structural.** When a value passes
between contexts — a `consumes` call argument, or a service's return value — it
need not name the same type on both sides; it MUST be **structurally compatible**:
the same `commons`-derived type, or an identical record or sum shape. A value's
type is then **projected** into the receiving context's namespace: its structural
shape is preserved and its brand is changed to the receiver's. A mismatch is
`bynk.boundary.structural_mismatch`. This projection is what lets a `commons`
type be shared across contexts without a shared nominal identity. A type
need not be exported to cross: a `commons` type the consumer does not `uses`,
and a type the providing context declares but does not export, cross the same
way (#1932). `exports` decides what a consumer may *name* and how it may build
or inspect a value, not whether the value may cross.

A returned value is projected **by name**: where the receiving context declares
a type of the same name, the value takes that type, so the two MUST be
structurally compatible (`bynk.boundary.structural_mismatch` at the call
otherwise). Where it declares none, the value keeps the providing context's
type, which the receiver can hold though it cannot name it, and pass on across
the boundary like any other value: back to the context it came from, or to
another consumed context whose parameter is structurally compatible (#1941). A value's type records its name, not the context it
came from, so when two consumed contexts declare differently shaped types of
one name, neither of which the receiver declares, a value of that name cannot
cross back: it is rejected, with a note saying why.

Two **refinements** match when their predicate *sets* are equal (v0.177). A
refinement is a conjunction of side-effect-free predicates, so their order
carries no meaning: `String where NonEmpty && MaxLength(10)` and
`String where MaxLength(10) && NonEmpty` are the same type and MUST match.
(Before v0.177 they were compared positionally and spuriously failed to.)
Duplicates are idempotent. The comparison is otherwise unchanged and remains
conservative: a *more* restrictive sending side is admitted into a more permissive
receiving one, but not the reverse. The same canonical form backs the
cross-context contract hash ([emission §7.3.4c](/book/spec/emission/#734c-the-contract-seam-v0177)),
so the matcher and the hash cannot disagree about what "the same refinement" is.

A context's `exports` clause controls what the boundary reveals: an
`exports transparent` type shares its structure with consumers — including
field-level construction — whereas an `exports opaque` type exposes only an
opaque handle — inspecting it from outside the owning context is rejected
(`bynk.context.opaque_inspection`), as is building a context-owned type through
its constructors (`.of`, `.unsafe`, a variant), or an opaque one with record
syntax, from outside (`bynk.context.external_construction`).

**Reading a field of a value from a consumed context** (#1948) follows what the
consumer may inspect. A field read on a value whose type is an `exports opaque`
type of another context is an inspection, rejected
(`bynk.context.opaque_inspection`) however the value was bound: from a call, a
parameter, or another value's field. A value whose type the providing context
declares but does not export (a service's unexported return type, or the
unexported type of an exported record's field) is *transparent but unnamed*:
the consumer cannot name the type, but its fields are read and typed against the
providing context's declaration, projected by name as for a returned value
above. An unknown field is `bynk.types.unknown_field` and a wrongly used one an
ordinary type error. Where more than one consumed context declares a type of
that name with different shapes, and the consumer declares none, the read cannot
be traced to one declaration and is rejected (`bynk.boundary.ambiguous_type`).

**An adapter's binding is a privileged constructor of its boundary types.** The
binding is host code: it sits outside Bynk's static semantics, and only the
emitted TypeScript types constrain it. For *transparent* records this is no
privilege — any consumer may construct them. The privilege bites on the stricter
kinds: a **refined** type's predicate is invisible to the TypeScript checker, so
a binding MUST construct refined values through the emitted validating `.of`
constructor and handle its `Result` — a raw cast or `.unsafe` would mint a value
the rest of the program trusts as validated without running the predicate — and
an **opaque** type, ordinarily constructible only by its defining unit, may be
built by the binding under the same emitted-constructors convention
([§7.3.6](/book/spec/emission/#736-adapters)). This requirement on bindings is a
convention enforced by review and the emitted constructors' shapes, not by a
`bynk.*` diagnostic — the binding is, by design, beyond the compiler's reach.

**Recursive types at boundaries** (#2613). A non-generic recursive type — a
recursive sum, or a record that recurses through an `Option`, a collection or a
sum (§6.1.4, §6.1.5) — is an ordinary boundary-legal type. It MAY cross a
context boundary, be a `store` field, be a field of an event payload, and pass
through `Json.encode` and `Json.decode`; its codec is recursive. A **generic**
recursive type (`type Node[T] = { value: T, next: Option[Node[T]] }`) is
rejected at a boundary (`bynk.generics.recursive_generic_at_boundary`): a
generic type crosses through a codec monomorphised per instantiation (ADR 0197,
0237), and a recursive instantiation has no finite set of them. It remains
usable inside a context.

**Nesting is bounded** (#2656). A boundary codec accepts a value of a
recursive type nested at most **256 levels** deep. The outermost value of a
recursive type is one level, and each value of a recursive type nested inside
it, through fields, variants, `Option`, collections or other recursive types, is
one more. A value of a non-recursive type is not counted, and below one the
count starts again; such a type occurs only a fixed number of times on any path
through a value, so the total nesting stays bounded. Decoding a value nested more deeply MUST fail as a
`StructuralMismatch` at the path where the bound is crossed, with `expected`
`a value nested at most 256 levels deep`, so a request body is refused with a
`400` and `Json.decode` returns an `Err`, rather than the decoder exhausting the
host's stack. Encoding such a value is a contract violation and MUST fault, as
encoding a non-finite `Float` does
([§7.2](/book/spec/emission/#72-targets)), so a context never sends or stores a
value its peer, or its own rehydration, would refuse.

**Collections are covariant in their element and value positions** (v0.20b):
a `List[T]` is usable where a `List[U]` is expected iff `T` is usable where
`U` is expected, and likewise a `Map`'s value type. A `Map`'s **key** type
MUST match exactly — widening a refined key to its base would split a map's
keys across two identities at lookup time.

At the type level, **purity is the absence of `Effect`**: an expression whose
type is an `Effect[T]` is effectful, and an effect may be performed only in an
effectful position. The well-formedness of this discipline is
[§5.5](/book/spec/static-semantics/#55-effects-capabilities--providers).

**The non-boundary value family.** Certain types are *built and consumed in
place* and MUST NOT appear in any **serialisable or boundary-crossing** position —
a `store` field, a context-boundary argument or return, or the JSON codec. These
are `Effect[T]`, **function types** (`A -> B`, §6.4a,
`bynk.types.function_at_boundary`), **`Stream[T]`**
(`bynk.types.stream_at_boundary`), **`Query[T]`** (`bynk.types.query_at_boundary`),
the held **`Connection[F]`** (`bynk.types.held_at_boundary`), and the opaque
key handles **`SigningKey`** and **`VerifyKey`** of `bynk.crypto`
(`bynk.types.key_at_boundary`): a key is held non-extractable by the platform and
lives for the call that imported it, so it is never persisted, logged or sent.
The confinement
**looks through collections** — a `List`/`Map` whose element or value is a
non-boundary type is itself rejected at a boundary. A held `Connection[F]` is the
single exception to "never stored": it may be stored, but **only** in
`Cell[Option[Connection]]` or `Map[K, Connection]`
([§5.4.2](/book/spec/static-semantics/#542-held-resource-linearity-v0102)); a
`Set`/`Log`/`Cache` rejects it (`bynk.held.unsupported_storage`). `Stream`,
`Query`, and `Connection` are additionally **not value-comparable** with `==`/`!=`
(`bynk.types.stream_not_comparable` / there is no query equality /
`bynk.types.held_not_comparable`).

A `Stream[T]` MAY appear in a **capability operation's** parameters and result,
since every provider runs in the caller's isolate and nothing in the signature is
serialised (#2623), as a `bynk.crypto` key may. A `Stream` MAY also be a
**record field or sum payload**. The declaring type, and any type that reaches it
through a field, payload, container or generic argument, is then
**stream-carrying**: it is confined exactly as `Stream` is, rejected at each
serialisable or boundary-crossing *use* with `bynk.types.stream_at_boundary`, and
has no JSON codec (`bynk.types.json_uncodable`). A stream is **single-pass**:
traversing it a second time faults. Other non-boundary types are still rejected
in a record field where it is declared.
