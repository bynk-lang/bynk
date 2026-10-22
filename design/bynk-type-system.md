# Bynk Type System — Specification

*Working draft — 13 May 2026*

> **Implementation status (refreshed October 2026, v0.309).** This is an
> aspirational specification and runs ahead of, or apart from, the compiler in
> places. The normative definition of what compiles is the language
> specification (`site/src/content/docs/book/spec/`), with the decision records
> (`decisions/`) behind it; treat "Settled" here as "settled in design". Where
> this document's names differ from what was built:
>
> - **Primitives.** The `PrimType` set in §1.1 is the built set, under the
>   compiler's names: `Int`, `Float` (ADR 0040), `String`, `Bool`, `Bytes`
>   (ADR 0142), `Instant` (ADR 0114), `Duration` (ADR 0112) and `Unit`, which
>   source writes `()`. This draft first named `Float` `Decimal` and `Instant`
>   `Timestamp`; it now uses the shipped names. Bynk has no exact-decimal
>   primitive (see §2.5.0).
> - **Held resources.** `Held[T]` is a closed kind, not a type you write; its one
>   built instance is `Connection[F]` (ADR 0130).
> - **Agent references.** `Ref[A]` is not built. An agent is addressed by
>   constructing it with its key and calling a handler, `Counter(id).bump()`,
>   from a service or from another agent.
> - **Tuples.** §2.7.6's tuples are not built and are not planned: bynk stays
>   nominal (ADR 0120 D1), and joins and grouping take an `into:` combiner.
> - **Effect inference.** Named declarations are declared, not inferred; only an
>   unannotated lambda's effect is read off its body (§2.8.4).
>
> Storage kinds (`Cell`/`Map`/`Set`/`Cache`/`Log`, v0.82–v0.97), the query
> algebra (v0.88–v0.94) and held connections (v0.100–v0.107) have shipped; see
> `archive/retired-tracks.md`.

## Status and scope

This document specifies Bynk's type system. It is a **working specification**, developed incrementally: sections marked "Open" identify decisions still to be made; sections marked "Settled" are committed and intended to be normative. The accompanying design notes (`bynk-design-notes.md`) document the rationale, alternatives considered, and architectural commitments that motivate the type system's shape; this specification is the prescriptive counterpart.

Bynk's type system has two parts:

1. A **core** that is essentially Hindley-Milner with closed sums and nominal records — well-trodden territory, included here for completeness and to fix the specific tuning choices.

2. **Architectural extensions** that implement Bynk's distinctive commitments: refinement at type declarations, opaque types, capability interfaces, storage type kinds, effects, held resources, and constrained refinement at other architectural points. The extensions are where most of the open work sits.

This document is read alongside the design notes, with cross-references where rationale matters. It is intended to be precise enough that a compiler implementer can read it and produce a type checker faithful to the language's commitments — and precise enough that, in writing it, ambiguities in the design surface and get resolved.

Conventions:

- Code blocks containing grammar use a BNF-like notation with `::=` for productions and `|` for alternation.
- Inline type-theoretic notation uses standard conventions: τ for types, σ for type schemes, α/β for type variables, Γ for type environments.
- "Settled" means the choice is committed in the design notes and reproduced here normatively.
- "Open" identifies decisions to be made; each open question is listed with the considerations that bound it.

---

## 1. Core type system

### 1.1 Type grammar — Settled

The grammar of types:

```
τ  ::=  α                                   -- type variable
    |   PrimType                            -- primitive type
    |   τ → τ                               -- function type
    |   C[τ₁, ..., τₙ]                      -- type constructor application
    |   { x₁: τ₁, ..., xₙ: τₙ }             -- structural record (anonymous, see §1.7)
    |   q.T                                 -- qualified type name from context q
    |   T                                   -- nominal type name

PrimType ::= Int | Float | String | Bool | Bytes
           | Instant | Duration | Unit
```

Type schemes generalise types over type variables:

```
σ  ::=  τ
    |   ∀α₁ ... αₙ. τ
```

A type scheme is a polymorphic type; a type is a monotype. Generalisation happens at let-bindings; instantiation at use sites (§1.3).

The `PrimType` set is fixed by the language. New primitive types require language work, not user definition. The current set covers numeric (`Int`, `Float`), text (`String`, `Bytes`), logical (`Bool`), temporal (`Instant`, `Duration`), and the unit type (`Unit`).

**Numeric primitives.** `Int` is an integer in the host's safe-integer domain, ±(2^53 − 1). `Float` is an IEEE 754 double (ADR 0040): a binary floating-point number, not an exact decimal, so `0.1 + 0.2` is not `0.3`. Both erase to the host `number`; the checker alone keeps them apart, and there is no implicit coercion between them (ADR 0041). A `Float` that crosses the wire must be finite. There is no exact-decimal primitive: quantities that need exact decimal arithmetic, money above all, are integers in their smallest unit (§2.5.0).

**Temporal primitives.** `Instant` (ADR 0114) is an integer count of milliseconds since the Unix epoch (1970-01-01T00:00:00Z). It carries the semantic of "an instant in time" but exposes only integer-like operations: comparison, arithmetic with `Duration`, equality. No calendar awareness — no year/month/day decomposition, no timezone handling, no parsing of date strings. It has no literal: an `Instant` comes from `Clock.now()` or from `Instant.fromEpochMillis(n: Int)`, and `t.toEpochMillis()` turns it back into an `Int`. `Duration` is a signed integer count of milliseconds, with arithmetic that composes with itself and with `Instant` (Instant + Duration = Instant; Instant - Duration = Instant; Instant - Instant = Duration; Duration + Duration = Duration; Duration * Int = Duration). The temporal primitives are the lowest layer; richer calendrical types (`Date`, `DateTime`, etc.) are library types built on these primitives in `bynk.time` and consumed by applications that need them.

**Binary primitive (built — ADR 0142).** `Bytes` is an immutable finite octet sequence — the representation for arbitrary binary data that `String` (UTF-8 text) cannot hold without corruption. It is erased to a host `Uint8Array` (the one base type not erased to `number`). There is **no source literal**: a `Bytes` is constructed by `Bytes.fromUtf8(s: String) -> Bytes` (total), `Bytes.fromBase64(s: String) -> Option[Bytes]` (partial — `None` on invalid base64), or `Bytes.empty() -> Bytes` (the zero value). Its usable surface is `length() -> Int`, `toBase64() -> String` (total), and `decodeUtf8() -> Option[String]` (partial). The standard-library K1 slice (#2016) adds `concat(other: Bytes) -> Bytes`, `slice(start: Int, end: Int) -> Bytes` (total; clamped like `String.slice`), `compare(other: Bytes) -> Int` (unsigned lexicographic, `-1`/`0`/`1`; not constant-time), `toHex() -> String` (lowercase) and `toBase64Url() -> String` (RFC 4648 §5, unpadded), all total, and the partial statics `Bytes.fromHex(s: String) -> Option[Bytes]` (either case, even length, nothing else) and `Bytes.fromBase64Url(s: String) -> Option[Bytes]` (padding optional, URL-safe alphabet only, canonical: non-zero spare bits are refused). **Equality is by content**, byte for byte — unlike the number-erased base types, whose `==` is host `===`, a `Bytes` compares by value, which is dedicated emitter codegen (a record or sum carrying a `Bytes` field gets correct equality when its field comparator threads that content-compare). `Bytes` is **equatable but not orderable** (no `<`, no `sortBy` key; `compare` is a method, not an order) and **not `Map`-keyable** (§storage; key on `toBase64()` — a `String`); it has no arithmetic. On the wire a `Bytes` **serialises as a base64 JSON string** and deserialises requiring a valid base64 string; it is a fully ordinary serialisable value — storable in any `store` kind and free to cross a `bundle` context boundary, the opposite of the non-serialisable `Stream`/`Connection`. (The erased `workers` cross-context wire path does not yet base64-encode a bare `Bytes`, so that one position is diagnosed as not-yet-supported; a `Bytes` inside a record crosses it fine via the record's typed codec.)

### 1.2 Term grammar — Settled in shape

Expressions:

```
e  ::=  x                                   -- variable
    |   c                                   -- literal (Int, String, Bool, ...)
    |   λx. e                               -- lambda
    |   e₁ e₂                               -- application
    |   let x = e₁ in e₂                    -- let binding (with generalisation)
    |   { x₁ = e₁, ..., xₙ = eₙ }           -- record construction
    |   e.x                                 -- field access
    |   match e with { p₁ => e₁ | ... }     -- pattern match
    |   Tag(e₁, ..., eₙ)                    -- sum variant construction
    |   e 'is' p                            -- pattern test (returns Bool, narrows on success; §2.3.6)
```

Concrete surface syntax (§16 of design notes) wraps these forms with specific keywords and punctuation; the abstract term grammar above is what the type checker operates on.

Open: the precise abstract grammar for let-bindings with multiple bindings, for blocks of statements (vs single expressions), for the `?` propagator and `<-` await operator (which interact with `Result` and `Effect` types respectively).

### 1.3 Inference: bidirectional — Settled

Type inference uses a **bidirectional** algorithm:

- **Checking mode**: when an expected type is propagated from context (e.g., from a function signature, a let-binding annotation, or a returned-value position), the checker verifies that the expression has that type.
- **Synthesis mode**: when no expected type is available, the checker computes the type from the expression's structure.

The two modes interact through annotations and through the structure of the language: annotations at handler boundaries, function signatures, and type declarations supply checking-mode expectations; expression interiors are largely synthesis-mode with localized checking at lambda parameters, record construction, and similar sites.

Why bidirectional over Algorithm W (or J): error messages are dramatically better because the checker has an expected type to point at when synthesis fails, and the language's annotation discipline (required at contract boundaries, inferred internally — §15 of design notes) naturally exercises both modes. The cost is more rule complexity and somewhat less inference power for unannotated code; both are acceptable given Bynk's design.

Generalisation: let-bound terms are generalised over free type variables not appearing in the surrounding environment (the standard HM let-polymorphism rule). Lambda-bound terms are *not* generalised — they remain monotypes within the lambda's body.

Instantiation: at use sites, polymorphic schemes are instantiated by replacing bound type variables with fresh type variables, which unification then refines.

### 1.4 Unification — Settled

First-order unification with the occurs check. Standard algorithm.

Unification of structural types unifies field-by-field for records and argument-by-argument for type constructors. Type constructors are compared by their nominal identity (i.e., `List[α]` unifies with `List[β]` by unifying α and β; `List[α]` does not unify with `Set[α]`).

Unification produces a substitution; the inference algorithm composes substitutions through the program.

### 1.5 The type environment — Settled in shape

The type environment Γ maps names to type schemes:

```
Γ  ::=  ∅ | Γ, x : σ | Γ, T : Kind
```

Names include term variables (with type schemes) and type names (with their kinds — see §2.7 for the kind system as it relates to storage types).

The environment is extended at let-bindings, lambda introductions, pattern bindings, and type declarations. Scoping is lexical.

Open: the precise interaction between the type environment and the bounded-context environment (§2.1). Cross-context references are qualified; the environment must track which context is currently being checked.

### 1.6 Out of scope — Settled

The following are explicitly **not** in v1:

- *Subtyping.* Polymorphism is parametric only.
- *Higher-rank polymorphism.* Only top-level rank-1 polymorphism via let-bound generalisation.
- *Higher-kinded types.* Type constructors are first-order; user-defined type constructors are not parameterised over kinds.
- *Row polymorphism.* Where "any record with these fields" would be useful, a nominal record type is declared.
- *Type classes / coherent instance resolution.* Capabilities are flat interfaces; no automatic dispatch.

Each is documented in §15 of the design notes with the rationale. Future addition is possible but requires architectural work; the type system is designed to be soundly extensible if pressure emerges.

### 1.7 Records: structural vs nominal — Open

The grammar includes anonymous structural record types `{ x₁: τ₁, ..., xₙ: τₙ }`. The design notes (§15) say nominal records are the rule and structural records "where you would want 'any record with these fields,' declare a transparent type for the projection."

Open question: does the language admit anonymous structural records *at all*, or are all records introduced via `type T = { ... }` declarations? If they exist, they're useful for short-lived intermediate values; if they don't, the language is more uniform (every record is named).

Provisional position: anonymous structural records are admitted *only* in narrow positions (pattern-match field projections, immediate construction passed as an argument) but cannot be named or stored. This keeps the architectural rule (records are nominal) while permitting some local convenience.

---

## 2. Architectural extensions

The extensions below add to the core. Each is documented with its grammar additions, its key typing rules, and the open questions remaining.

### 2.1 Bounded contexts and visibility

#### 2.1.1 The context as a scope — Settled in shape

A bounded context is a named scope containing type declarations, capability declarations, agent declarations, service declarations, and function declarations. The grammar:

```
context-decl ::= 'context' QualifiedName '{' decl* exports? '}'

decl ::= type-decl | capability-decl | agent-decl | service-decl 
       | fn-decl | event-decl | test-decl | invariant-decl
       | consumes-decl | provides-decl

exports ::= 'exports' 'opaque' '{' Name+ '}'
          | 'exports' 'transparent' '{' Name+ '}'
          | 'exports' 'private' '{' Name+ '}'
          | (multiple `exports` clauses combine)
```

Each declared name belongs to its context. Exports declare which names are visible across the boundary and how (opaque, transparent, or private — though "private" is the default and rarely needs explicit listing).

#### 2.1.2 Imports and cross-context references — Open

A context declares two kinds of dependencies, with distinct keywords reflecting distinct relationships:

```
import-decl ::= 'consumes' QualifiedName     -- behavioural dependency on another context
              | 'uses' QualifiedName         -- vocabulary import from a commons (§2.1.4)
```

- `consumes` declares a behavioural dependency on another bounded context. The consuming context can call the consumed context's handlers, observe its events, and reference its exported types. This is a runtime-coupling relationship; cycles are forbidden.
- `uses` declares an import from a `commons` — a flat, behaviour-free unit of types and pure functions (§2.1.4). The using context gets the commons' exported types and functions in scope. No runtime coupling; cycles among commons are permitted.

Inside the consuming context, types from a consumed context or used commons can be referenced by their qualified name (`commerce.money.Money`) or, if imported into scope (mechanism TBD), by their bare name (`Money`).

Open questions:

- *Aliasing on import.* `consumes commerce.payment as payment`? `uses commerce.money as money`? The mechanism for bringing names into local scope without full qualification is not yet committed.
- *Cyclic dependencies among contexts.* Forbidden statically; the context dependency graph must be acyclic. Cycles among commons are permitted (no runtime coupling).
- *Re-export.* If context A consumes B and exports a type derived from B's types, what's the rule for whether B's types are transitively visible from A's consumers? Probably: A's consumers see A's exports; B's types are visible only through A's exported surface.

#### 2.1.3 Visibility and construction — Settled in shape

Bynk enforces **full encapsulation of context-owned types**: a value of a type defined in a context can only be constructed within that context. Export visibility governs what consumers can *see* of the type's structure; it never grants construction authority. The principle is that bounded contexts are sealed against external minting — cross-context interaction happens through service operations and events, not through external construction of the context's types.

**Exports are a context mechanism, not a commons mechanism.** Commons types and functions don't have visibility levels because they don't cross a context boundary in the same way — they are mixed into each using context's scope (§2.1.4) rather than exported across a boundary. The mechanisms answer different architectural needs: *mixin* shares vocabulary into a context's local language; *export* governs the contract a context offers to callers of its services. A context typically uses both — mixing in commons for shared vocabulary, exporting its own types to make its service boundary tractable for callers.

At every site referencing a name `T` from context `q`:

1. The site's current context is `c` (lexically determined).
2. If `c = q`, the reference is unrestricted: T's full structure is visible, construction by literal is permitted, pattern matching destructures T's representation.
3. If `c ≠ q` and `T ∈ exports(q, opaque)`: the reference is valid; T may be used as an argument, returned, stored, compared by `==`; T cannot be inspected (no field access, no pattern matching) and cannot be constructed.
4. If `c ≠ q` and `T ∈ exports(q, transparent)`: the reference is valid; T's shape is visible — fields can be accessed, patterns can destructure, sums can be matched on. T **cannot be constructed**. Construction outside the defining context is a compile error.
5. If `c ≠ q` and `T ∉ exports(q)`: compile error — T is not exported from q.

The visibility distinction (opaque vs transparent) is about read-side access. Opaque hides the shape (T is a token); transparent reveals the shape (T is readable data). Neither grants the right to mint new values of T.

**How consumers obtain values of context-owned types.** Three paths:

- *Receive them from the context*: handler return values, event payloads, awaited capability operation results.
- *Boundary deserialisation*: the framework deserialises wire inputs into the context's types using the context's validation rules. This is logically construction *inside* the context — the deserialiser is part of the boundary service.
- *Cross-context service operations*: the context exposes operations (via `provides Capability` or via agent handlers) that perform the construction internally and return references or values.

What consumers do not do: construct values of foreign types directly, even via "factory functions" or constructor methods. The construction surface stays inside the owning context's lexical boundary.

**Cross-context error and outcome translation.** The idiomatic pattern of "pattern-match foreign error, construct local error" is fully consistent with the principle: pattern-matching is read-side (admitted on transparent foreign types); the local error is constructed in the current context (where it's defined; admitted by rule 2). This is the Anti-Corruption Layer expressed in the type system.

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

**Diagnostic when violated.**

```
bynk.types.external_construction:
  cannot construct values of type `Voucher` outside its defining context
  type `Voucher` is owned by context `commerce.vouchers`
  this construction is in context `commerce.orders`
  
  cross-context interaction goes through service operations or events.
  to obtain a Voucher reference, call a service operation exposed by `commerce.vouchers`.
```

**Exempt types.** Three categories of types have no context restriction on construction:

- *Built-in language types*: `Option`, `Result`, `List`, tuples, the primitive types. They have no defining context.
- *Commons-defined types*: types defined in a `commons` declaration are admitted for construction in any context that `uses` the commons. The mixin semantics (§2.1.4) make commons types local to each using context, so construction is in-scope.
- *Handler-internal types*: types declared inside a handler body or function body (rare but admissible).

Open: the precise rule for visibility of type *parameters* of exported types (if `Inventory(sku: Sku)` is exported opaquely, is `Sku` necessarily visible? Probably yes); the exact policy for test code (whether tests get an exemption to the construction rule for mocking purposes, or whether mock provider patterns route through the owning context).

#### 2.1.4 Commons — Settled in shape

A **commons** is a flat, peer-of-contexts declaration that bundles shared types and pure functions, *mixed into* any context that `uses` it. Commons capture the **shared kernel** pattern from DDD — a deliberately-scoped set of definitions that participate in multiple contexts' ubiquitous language without belonging to any of them.

**Commons doesn't export — it mixes in.** Unlike contexts, commons declarations don't have visibility levels (no `opaque` or `transparent` exports). Every type and function declared in a commons becomes available in every using context's scope. The distinction is architectural: a context's `exports` clause governs the contract callers see at the service boundary (what they can hold as tokens, what they can read structurally); a commons's declarations are the shared vocabulary of the contexts that use it. The two mechanisms answer different questions:

- *"What vocabulary do I want as part of my context's language?"* → `uses` a commons. Mixin brings declarations into scope.
- *"What types do I want callers of my services to be able to interact with?"* → `exports` from a context. Visibility determines whether callers hold tokens or read shapes.

A context typically uses both: mixing in commons for shared vocabulary while exporting its own context-owned types selectively to make its service boundary tractable. Mixin governs vocabulary in; exports governs contract out.

**Mixin semantics.** When a context declares `uses commerce.money`, the commons's declarations are *brought into the context's scope* as if declared locally. Inside the using context, `Money`, `CurrencyCode`, and any pure functions defined in `commerce.money` are local types and local functions of the using context. This has three consequences:

- *Construction is admitted in each using context.* Because commons types become local types via mixin, the construction rule from §2.1.3 ("construction in defining context") admits them — each using context becomes a defining context for its mixed-in commons types. Construction of `Money { minorUnits: 1000, currency: gbp }` inside `commerce.orders` is in-scope construction of a local type.
- *Each using context has its own nominal type derived from the mixin.* `commerce.orders.Money` and `commerce.payment.Money` are distinct nominal types that share a structural shape (because both derive from the same commons declaration). This is the type-system reading of the source-level mixin compilation model.
- *Structural compatibility across contexts is automatic.* Two contexts that mix in the same commons have structurally identical declarations of the commons types. Values of those types flow across context boundaries through structural projection: the sending context serialises the value as data; the receiving context constructs its own nominal type from the data, applying its local refinements at the construction boundary.

The mixin model resolves what would otherwise be a tension: commons types need to be constructible in any context that uses them (because they're shared vocabulary), but the construction rule says types are constructed in their defining context. The resolution is that commons doesn't *define* in the way a context does — it provides declaration material that's incorporated into each user's scope. Every user becomes a defining context for the mixed-in declarations. The architectural property — values mint where their meaning lives — holds, because for commons types meaning *is* the shape, and the shape is everywhere a context mixes the commons in.

Declaration grammar:

```
commons-decl ::= 'commons' QualifiedName '{' commons-decl-body* '}'

commons-decl-body ::= type-decl
                    | fn-decl                  -- function bodies are restricted; see below
                    | commons-uses-decl

commons-uses-decl ::= 'uses' QualifiedName     -- commons may use other commons
```

Note: no `exports` production. All top-level declarations in a commons are part of the mixin and are available in every using context.

Examples:

```
commons commerce.money {
  type CurrencyCode = String where Matches("[A-Z]{3}")
  type Money        = { minorUnits: Int, currency: CurrencyCode }

  fn addMoney(a: Money, b: Money) -> Result[Money, CurrencyMismatch] {
    if a.currency != b.currency { Err(CurrencyMismatch(a.currency, b.currency)) }
    else { Ok(Money { minorUnits: a.minorUnits + b.minorUnits, currency: a.currency }) }
  }

  type CurrencyMismatch = { left: CurrencyCode, right: CurrencyCode }

  exports transparent { CurrencyCode, Money, CurrencyMismatch }
}

commons bynk.time {
  uses bynk.primitives    -- if another commons existed at that name
  
  type Date     -- opaque, wrapping Instant
  type DateTime
  
  fn now() -> ???    -- NOT permitted: would require a Clock capability
}
```

**The constraints that make a commons safely shareable:**

- No `agent` declarations.
- No `service` declarations.
- No `capability` declarations.
- No `provides` declarations.
- No `store` fields (commons have no state).
- No `event` declarations.
- No `invariant` declarations (no state to invariant).
- No `consumes` clauses (commons don't depend on contexts).
- Function bodies must be **pure**: no `<-` operator, no `given` clauses, no calls to operations that return `Effect[T]`. They operate on their typed inputs and return values; nothing more.

The compiler enforces these constraints. A commons that violates any of them is a compile error with a diagnostic pointing at the offending declaration and a suggestion (e.g., "this function uses `given Clock`; capabilities belong in a context, not a commons — consider moving this to an appropriate context or removing the capability use").

**What a commons may contain:**

- All forms of type declaration: refined value types, opaque types, transparent records, closed sums, generic types.
- Pure functions over those types.
- `exports opaque`, `exports transparent`, `exports private` clauses — same mechanism as contexts.
- `uses` of other commons (cycles permitted; no runtime coupling).

**Naming and the architectural flat-ness.** A commons has a qualified name like `bynk.time`, `commerce.money`, or just `money`. The dotted hierarchy is purely organisational — it groups names in the project's filesystem and signposts logical clustering, but carries no architectural meaning. A commons named `commerce.money` is not "for" `commerce.*` contexts any more than for any other context; any context can `uses commerce.money` regardless of its own name. The naming hierarchy reflects organisation; the language imposes no implicit relationships from it.

This sits with DDD's strict view that bounded contexts are flat. The dotted naming captures subdomain organisation; the commons captures shared kernel. Neither imposes containment.

**Cycles among commons.** Two commons may reference each other directly or transitively. Pure types and pure functions have no runtime, no execution order, no startup ordering — so mutual references don't pose the problems that context cycles would. The compiler permits cyclic `uses` among commons; a context's `consumes` graph remains acyclic.

**File organisation.** Commons live at file paths matching their qualified name, the same convention as contexts (§19 of design notes). The compiler distinguishes a commons from a context by the keyword inside the file (`commons commerce.money { ... }` versus `context commerce.payment { ... }`).

```
src/
├── bynk/
│   └── time.bynk                       -- commons bynk.time
├── commerce/
│   ├── money.bynk                      -- commons commerce.money
│   ├── inventory.bynk                  -- context commerce.inventory
│   └── payment.bynk                    -- context commerce.payment
└── hotel/
    ├── identifiers.bynk                -- commons hotel.identifiers
    ├── bookings.bynk                   -- context hotel.bookings
    └── rooms.bynk                      -- context hotel.rooms
```

**Relationship to capabilities.** Commons and capabilities are orthogonal axes of reuse:

- *Capabilities* are behavioural contracts. They have providers (declared via `provides` in contexts). They're imported via `given` clauses. They answer "how does my code talk to the outside?"
- *Commons* are pure types and pure functions. They have no providers. They're imported via `uses`. They answer "what does our code talk about?"

A context typically uses both: `uses commerce.money` for the vocabulary; `given Clock, Payments` for the behaviour. Both kinds of import appear at the top of the context declaration.

**Why commons rather than putting shared types in some "shared" context.** The earlier worked examples used a `commerce.shared` context that contained only types. That worked but conflated two distinct things: behavioural contexts and shared-vocabulary modules look the same linguistically, even though they play architecturally different roles. The `commons` keyword makes the distinction explicit at the declaration site, the constraint set (no behaviour) enforces the discipline, and the `uses` import keyword signals the relationship — different from `consumes`, with different cycle rules and different evolution implications.

Open: re-export from one commons to another (whether and how a commons that `uses` another can expose the used types as its own exports); whether commons can be parameterised (probably no for v1; if needed, parameterisation lives in generic types within a commons); diagnostic messages for the constraint violations.

#### 2.1.5 Test contexts — Settled in shape

Bynk has three kinds of top-level declarations: `context` for bounded contexts, `commons` for shared vocabulary, and `test` for testing. A **test context** declares an explicit testing relationship with a target — either a context or a commons — and inherits specific privileges relative to that target.

Declaration grammar:

```
test-context-decl ::= 'test' QualifiedName '{' test-context-body* '}'

test-context-body ::= test-decl
                    | import-decl
                    | provides-decl
                    | fn-decl              -- helper functions for tests (test-scope pure)
                    | type-decl            -- test-only types (rare; typically not needed)

test-decl ::= 'test' StringLiteral tag-list? '{' stmt* '}'

tag-list ::= '#' identifier ('#' identifier)*
```

The `QualifiedName` after `test` names the target — either an existing context or a commons. `test commerce.orders` declares the test context for `commerce.orders`; `test commerce.money` declares the test context for the `commerce.money` commons. The target must exist (compile error otherwise) and there is at most one test context per target (multi-file expansion via directories allows multiple files contributing to the same test context, the same way regular contexts can be multi-file).

**Test-context privileges relative to the target.** A test context for target T has elevated capabilities specifically with respect to T's types and internals:

- *Direct construction of T's types.* The construction rule in §2.1.3 is relaxed for types defined in T; the test context can write `Cart { ... }`, `OrderError.PaymentDeclined`, `Mock[OrderId]`, or any other construction expression for a T-defined type.
- *Access to T's private items.* Private types and internal helpers in T are visible to the test context. This is the white-box testing case: a test can exercise an internal function, inspect agent state directly, verify private invariants.
- *Capability substitution via `provides`.* The test context can `provides` capabilities that the system under test consumes. Mock implementations are linked at test-build time, replacing whatever providers would be used in production.

These privileges are *bounded to T*. For other contexts the test context consumes (e.g., `test commerce.orders` consuming `commerce.inventory` and `commerce.payment`), normal cross-context rules apply: the test context cannot directly construct their types and cannot access their private items. For those, the path is `Mock[T]`.

**Mock[T] — test-time value construction.** `Mock[T]` is a language-level construct admitted only in test contexts. It produces a value of type T:

- *For refined primitive types*: generates a value satisfying the refinement (`Mock[Sku]` produces a valid Sku string matching the type's refinement).
- *For sum types*: defaults to the first declared variant, or accepts an explicit variant — `Mock[ReserveOutcome](Reserved(Mock[ReservationId]))`.
- *For records*: defaults each field to `Mock[FieldType]`, or accepts overrides — `Mock[Cart](items: [...], total: ...)`.
- *For opaque types*: generates a synthetic token representation satisfying the type's identity property (two `Mock[OrderId]` calls produce distinct values).
- *For transparent records and sums*: the shape is known to the compiler, so generation is straightforward; overrides are taken from the call.

**Mock[T] parameterisation forms.** The `Mock[T]` expression accepts arguments to override defaults:

- *Bare*: `Mock[T]` — fully generated. Refinement-respecting; first-variant default for sums; field-default-mock for records; synthetic token for opaque types.
- *Literal pin*: `Mock[T](literal)` — for refined primitives, pins the value (`Mock[Sku]("ABC123")`, `Mock[Quantity](5)`). The literal must satisfy the type's refinement; a compile error otherwise.
- *Variant pin*: `Mock[T](Constructor(args))` — for sum types, specifies the variant and its payload (`Mock[ReserveOutcome](Reserved(Mock[ReservationId]))`, `Mock[Status](Cancelled(at: now))`).
- *Record overrides*: `Mock[T] { field: value, ... }` — for record types, overrides specific fields while defaulting the rest (`Mock[Cart] { items: [item1, item2] }` — `total` is still mocked).

The forms compose naturally with the surrounding type system: record-override is the same syntax as record construction; variant-pin is variant construction; literal-pin is the refined type's constructor. The compiler treats them uniformly — `Mock[T]` is the operator that elevates construction to test scope.

Mock values are typed identically to real values (a `Mock[Voucher]` has type `Voucher`) but are distinguished by the compiler for tooling and diagnostic purposes. Outside test contexts, `Mock[T]` is a compile error.

**Capability substitution syntax.** Test contexts provide capabilities the system under test consumes using the same `provides` keyword as production, but with operation definitions inline:

```
provides Activities {
  record = (label, duration) => Ok(Ok(Mock[ActivityId]))
}
```

Each operation defined in the capability is given an implementation as a closure with the operation's signature. The closure body can use `Mock[T]` for any types in scope. Build-time linking substitutes these implementations for production providers when running tests.

Per-test overrides nest naturally: a `provides` declaration inside a `test "..."` block overrides any context-level `provides` for the duration of that test.

```
test fitness.workout {
  uses     fitness.units
  consumes platform.activity

  provides Clock = FixedClock(2024-01-15T10:00:00Z)
  provides Activities {
    record = _ => Ok(Ok(Mock[ActivityId]))
  }

  test "first scenario uses the context-level providers" #unit {
    -- Clock and Activities as declared at the context level
  }

  test "second scenario overrides Clock" #unit {
    provides Clock = SequentialClock([
      2024-01-15T10:00:00Z,
      2024-01-15T10:45:00Z,
    ])
    -- Clock here is SequentialClock; Activities still uses the context-level provider
  }
}
```

Capability names in `provides` declarations may be bare (`provides Activities`) when the capability is unambiguously in scope from a `consumes` or `uses` declaration, or qualified (`provides platform.activity.Activities`) when disambiguation is needed.

**Call capture on substituted providers.** Every capability whose implementation is provided in a test context automatically captures its calls. The compiler generates a sum type from the capability declaration: for a capability with operations `record(label: String, duration: Duration) -> Effect[Result[ActivityId, ActivityError]]`, the generated type is `ActivitiesCall` with a variant `Record(label: String, duration: Duration)` (and similar variants for any other operations).

```
capability Activities {
  record(label: String, duration: Duration) -> Effect[Result[ActivityId, ActivityError]]
  query(filter: ActivityFilter) -> Effect[List[ActivitySummary]]
}

-- Compiler generates:
type ActivitiesCall =
  | Record(label: String, duration: Duration)
  | Query(filter: ActivityFilter)
```

Access to captured calls is via `.calls` on the capability name inside a test:

```
test "completing records exactly one activity" #unit {
  provides Activities {
    record = _ => Ok(Ok(Mock[ActivityId]))
  }

  let workout = Workout(WorkoutId.fresh())
  <- workout.start()?
  <- workout.complete()?

  assert Activities.calls.length == 1
  assert Activities.calls[0] is Record(label, _) && label == "workout"
}
```

`.calls` is `List[ActivitiesCall]` (with the generated sum type), preserving call order (chronological by invocation). Captured calls reset between tests — each test starts with an empty call log unless explicitly stated otherwise.

**White-box state access.** Test contexts have privileged access to their target context's agent state. From within `test commerce.orders`, the expression `someOrderAgent.fieldName` reads the underlying storage (Cell value, Map contents, etc.) directly, without going through a handler. This is the white-box testing privilege:

```
test fitness.workout {
  test "starting transitions agent to Active" #unit {
    let workout = Workout(WorkoutId.fresh())
    <- workout.start()?

    -- Direct read of agent state from the test context
    assert workout.state is Active(_, _)
  }
}
```

In production code, agent state is private to the agent — external readers go through query handlers. In test contexts, the target's state is readable directly. Writing agent state from outside a handler remains forbidden even in test contexts: `someAgent.field := value` is a compile error. The privilege is observation, not mutation.

For storage types that aren't `Cell`, the access patterns mirror the type's interface but in synchronous form: `someAgent.someMap.get(key)`, `someAgent.someSet.contains(value)`, `someAgent.someLog.collect`. The synchronous shape reflects that test access is direct memory access on the agent's state, not a handler invocation.

**Worked example.**

```
test commerce.orders {
  uses commerce.money
  consumes commerce.inventory, commerce.payment
  
  -- Capability substitution at the test-context level applies to all tests
  provides Clock = FixedClock(2024-01-15T12:00:00Z)
  
  test "placement succeeds when all dependencies cooperate" #unit {
    -- Direct construction (this IS the test for commerce.orders)
    let user = UserId.of("user_test_001")?
    let cart = Cart {
      items: [CartItem { sku: Mock[Sku], qty: Quantity.of(3)?, unitPrice: Money.of(1000, gbp) }],
      total: Money { minorUnits: 5000, currency: gbp },
    }
    
    -- Per-test capability substitution; calls captured automatically
    provides Inventories {
      reserve = _ => Ok(Mock[ReserveOutcome](Reserved(Mock[ReservationId])))
      release = _ => Ok(())
    }
    provides Payments {
      authorise = _ => Ok(Mock[AuthId])
    }
    
    let result <- Order.place(user, cart)
    
    assert result.isOk
    assert result.unwrap().total == cart.total
    assert Inventories.calls.length == cart.items.length
    assert Payments.calls.length == 1
  }
  
  test "placement fails when inventory unavailable" #unit {
    let user = Mock[UserId]
    let cart = Mock[Cart]
    
    provides Inventories {
      reserve = _ => Ok(Mock[ReserveOutcome](InsufficientStock(available: 0, requested: Quantity.of(1)?)))
    }
    
    let result <- Order.place(user, cart)
    
    assert result.isErr
    assert result.unwrapErr() is OrderError.OutOfStock(_)
  }
}
```

A reader can identify everything from the declaration: it's the test context for `commerce.orders`; it uses commerce.money; it consumes inventory and payment for mock construction; it provides Clock at the context level (all tests share it) and substitutes other providers per-test where they vary. The tests construct cart/user freely (commerce.orders types) and Mock the foreign types.

**File organisation.** Test contexts follow the same naming-to-path convention as contexts and commons (§19 of design notes):

```
src/
├── commerce/
│   ├── orders.bynk              -- context commerce.orders
│   ├── orders.test.bynk         -- test commerce.orders
│   ├── inventory.bynk           -- context commerce.inventory
│   └── inventory.test.bynk      -- test commerce.inventory
└── hotel/
    ├── bookings.bynk
    └── bookings.test.bynk
```

The `.test.bynk` suffix is a discovery convention for the test runner; the actual test-for relationship comes from the `test QualifiedName` declaration inside the file. Test contexts can be multi-file via directory expansion: `commerce/orders.test/place.test.bynk` and `commerce/orders.test/cancel.test.bynk` both contribute to `test commerce.orders`.

**Test commons.** A test context may want to share fixtures and test-helper functions with other test contexts. The natural construct is `test commons` — a commons declaration with test privileges:

```
test commons commerce.test.fixtures {
  uses commerce.money
  
  fn standardCart() -> Cart {
    Mock[Cart](items: [
      Mock[CartItem](sku: Mock[Sku], qty: Quantity.of(2)?, unitPrice: Money.of(1000, gbp))
    ])
  }
  
  fn validUser() -> UserId { UserId.of("user_test")? }
}
```

Other test contexts `uses commerce.test.fixtures` to access the shared helpers. Test commons differ from regular commons only in admitting `Mock[T]` in their function bodies.

Open: the precise syntax for capability substitution (`MockInventories({...})` is illustrative); whether `Mock[T]` accepts a literal in place of generating (e.g., `Mock[Sku]("ABC123")` to pin the value); how property-based testing integrates (likely `forall x: Mock[T] { ... }` quantifying over generated values); diagnostic messages for misuse.

### 2.2 Opaque types

#### 2.2.1 Declaration — Open

The current sketch:

```
opaque-type-decl ::= 'type' Name             -- representation supplied by constructor functions
                   | 'type' Name '=' τ       -- with explicit representation
```

Open: how the representation is declared. Three plausible options:

1. *Separately by constructor functions.* `type OrderId` declares the type; functions in the same context like `fn OrderId.fresh() -> OrderId given Random` provide construction. The representation is implicit in the constructor's body.
2. *With explicit representation that becomes opaque on export.* `type OrderId = String` declares the representation; if exported opaquely, external consumers see only the nominal type. This is more uniform with transparent types.
3. *Hybrid.* Allow both forms.

Provisional position: option (2). It's more uniform and makes the representation grep-able from inside the owning context. The opacity is a visibility property, not a structural one.

#### 2.2.2 Identity and equality — Settled

Opaque types are nominally distinct from their representations and from each other. `OrderId` and `UserId`, both `= String` underneath, are distinct types.

Equality on opaque types is **structural equality of the underlying representation** (§10 of design notes). Two `OrderId` values constructed from the same input compare equal. The compiler implements `==` on an opaque type by deferring to `==` on its representation.

Hashing follows the same rule: opaque values hash according to their representation.

#### 2.2.3 Operations — Settled in shape

Inside the owning context, all operations available on the representation are available on the opaque type. Outside, only operations the context explicitly exports (functions, methods on the type, equality, hashing) are available.

Open: the precise mechanism for declaring "exported operations" on an opaque type. Probably: any function in the same context whose signature mentions the opaque type is callable on values of that type from outside, provided the function is itself exported. So exporting an opaque type also implicitly exposes its exported-companion-function surface.

### 2.3 Closed sums

#### 2.3.1 Declaration — Settled

```
sum-type-decl ::= 'type' Name [type-params] '=' variant ('|' variant)*

variant ::= Tag                                     -- nullary variant
          | Tag '(' field (',' field)* ')'          -- variant with fields

field ::= τ                                         -- positional
       | Name ':' τ                                 -- named
```

Examples:

```
type Status = Pending | Placed | Cancelled
type Outcome = Reserved(ReservationId) | InsufficientStock(available: Int, requested: Quantity)
type Option[T] = None | Some(T)
type Result[T, E] = Ok(T) | Err(E)
```

A variant uses either all-positional or all-named fields, not a mix within a single variant. Mixing across variants of the same type is permitted.

Variant type parameters distinct from the parent type's parameters are not supported in v1.

#### 2.3.2 Construction — Settled

Nullary variants are constructed by bare name: `Pending`, `None`, `Created`. The compiler resolves the constructor to its declared type by context (or fails with an ambiguity error if multiple types have a nullary variant of the same name in scope).

Variants with positional fields are constructed with positional arguments: `Some(x)`, `Reserved(rid)`, `Ok(receipt)`.

Variants with named fields can be constructed in either form:

- *Positional*: `InsufficientStock(5, qty)` — fields supplied in declaration order.
- *Labelled*: `InsufficientStock(available: 5, requested: qty)` — fields supplied by name.

Labelled construction is required when the variant has fields of the same type whose order is non-obvious; the compiler errors on positional construction in those cases and suggests the labelled form. Style guidance is to prefer labelled construction for variants with more than one field.

#### 2.3.3 Exhaustive matching — Settled

`match` expressions against a sum type must cover every variant. The compiler errors on inexhaustive matches with a list of missing patterns. A wildcard `_` covers remaining cases.

```
match outcome {
  Reserved(rid)                           => ...
  InsufficientStock(available, requested) => ...
}
```

If any variant is uncovered and there's no wildcard, compile error.

#### 2.3.4 Pattern matching — Settled in shape

The pattern grammar:

```
p ::= x                                         -- variable binding
   |  _                                         -- wildcard
   |  c                                         -- literal
   |  Tag                                       -- nullary variant
   |  Tag(p₁, ..., pₙ)                          -- variant with positional patterns
   |  Tag(name₁: p₁, ..., nameₙ: pₙ)            -- variant with named patterns
   |  { x₁: p₁, ..., xₙ: pₙ }                   -- record pattern
   |  { x₁: p₁, ..., xₙ: pₙ, .. }               -- record pattern with rest
   |  p 'where' refinement-predicate            -- refined pattern
   |  p '|' p                                   -- or-pattern (left-associative)

match-arm ::= p ('if' guard-expression)? '=>' body
```

*Literal patterns and primitive scrutinees* (v0.130). A `match` ranges over
*either* a sum / `Result` / `Option` (variant-kind) *or* a primitive `Int` /
`String` / `Bool`, including a refinement over one of those bases (literal-kind).
A literal pattern `c` — an integer (optionally negated), a string, or a boolean,
the closed set of [ADR 0001](decisions/0001-literal-admission-set.md); no
`Float`, no `()` — is admitted only against a literal-kind scrutinee and matches
by value equality. Literal and variant patterns do not mix in one `match` (a
literal cannot match a sum, and vice-versa). Exhaustiveness: `Int` and `String`
are unbounded, so a literal-only `match` needs a wildcard `_` arm; `Bool` is
complete once both `true` and `false` appear (or a wildcard does). A repeated
literal arm is rejected. Literal patterns are `match`-only — a literal on the
right of `is` is rejected, since `is` tests a value's type/refinement, not
equality (use `==`). Or-patterns (below) are implemented (#474); the
remaining forms — record and tuple — are settled in shape but not yet
implemented.

*Refined patterns* (#472). `p 'where' refinement-predicate` guards a pattern
with a runtime predicate test, reusing the closed refinement-predicate
catalogue (§2.5.2) verbatim. v1 admits only `_ where predicate` — the inner
form is always a wildcard; a binding inner (`x where predicate`) is deferred.
A refined pattern is admitted only against a literal-kind scrutinee (`Int` or
`String`; `Bool` has no applicable predicate, and `Float` stays rejected like
any other literal-kind match, deferred). It is a **guard, not a narrowing**:
matching a refined arm does not change the static type of anything in its
body — that waits on refinement propagation (§2.5.4), still an open question.
Like an `if` guard, a refined arm is never exhaustive on its own (the
predicate might fail at runtime), so a refined-only arm set still needs a
wildcard `_` arm. Refined patterns are `match`-only — one on the right of
`is` is rejected, the same posture as a literal pattern, since `is` already
has its own refinement check over a *named* refined type (ADR 0007). A
refined pattern composes with or-patterns only as the *whole* pattern's
outer wrapper — `(A | B) where P` refines the alternation as a unit — never
as one alternative among others: `A where P | B` has no such reading, since
a `where` always attaches to the entire preceding pattern, alternation
included.

Variant patterns with named fields support both positional and labelled forms:

- `InsufficientStock(available, requested)` — positional, binds by position
- `InsufficientStock(available: a, requested: r)` — labelled, binds by name with a renamed local
- `InsufficientStock(available: _, requested: r)` — labelled with discard

When a variant has a single named field, the positional pattern `Toggle(v)` binds `v` to the field's value (a one-field variant cannot be ambiguous about which field is being bound). Sugar: `Toggle(v)` is interchangeable with `Toggle(value: v)` when the variant is `Toggle(value: Bool)`.

*Guards* attach a boolean condition to a pattern; the arm matches only if the pattern matches *and* the guard evaluates to true:

```
match existing {
  Some(current) if current.value == flag.value => Unchanged
  Some(current) => Updated(previous: current.value)
  None => Created
}
```

Guards are evaluated after the pattern binds; they can reference the bindings introduced by the pattern. Exhaustiveness checking treats guarded arms as not exhaustive on their own (the guard might fail); the compiler errors if no unguarded arm covers a variant.

A map's `.entries` query (§2.7.3, ADR 0184) exposes each entry as a nominal
`MapEntry[K, V]` record — bynk has no tuple/pair type (ADR 0120), so a key and
its value travel as a *named* record, read with `.key`/`.value`, not a
positional pattern:

```
flags.entries.all((e) => e.key == e.value.name)
```

The lambda parameter `e` is an ordinary `MapEntry[K, V]` binding; the whole
single-argument query vocabulary applies to `.entries` unchanged.

*Or-patterns* (`p₁ | p₂`) match either alternative. The pattern alternation operator `|` is left-associative and distinct from boolean OR `||`. Inside any pattern position, `|` is alternation; outside, it isn't a valid expression operator, so there's no syntactic ambiguity.

For an or-pattern `p₁ | p₂ | ... | pₙ` to be well-typed, three rules apply:

- *Same set of bindings.* Each alternative must bind the same names. `Held(g, r, _, _, _) | Confirmed(_, r, _, rsv, _)` is a compile error because `g` is only bound in the first alternative and `rsv` only in the second.
- *Same type for each shared binding.* A name bound in multiple alternatives must have the same type across all of them, including refinement. Different refinements (`Int where InRange(0, 100)` in one alternative versus `Int` in another) are a compile error; the user resolves either by widening to a wildcard or by splitting into separate arms. The rule rejects silent refinement loss.
- *Same value type.* The pattern matches a single type, typically a sum type whose alternatives cover multiple variants. Primitive patterns and record patterns also compose under `|`.

Wildcards don't bind, so they can appear in different positions across alternatives. This is the canonical use case: two or more variants with similar shapes where only some fields are needed.

```
match state {
  Held(_, r, _, rsv, _) | Confirmed(_, r, _, rsv, _) => {
    -- r: RoomId, rsv: ReservationId in scope; bindings consistent across both
    <- Rooms(r).release(rsv)
    ...
  }
  Pending | Cancelled(_, _) => ()
}
```

Or-patterns compose with guards:

```
match command {
  Login(u, _) | Register(u, _) if u.isAdmin => "admin path"
  Login(_, _) | Register(_, _)              => "user path"
}
```

An or-pattern covers all its alternatives for exhaustiveness checking. So `Held(_, _, _, _, _) | Confirmed(_, _, _, _, _)` covers two variants of `BookingState`; the compiler tracks coverage across arms and reports remaining uncovered variants.

#### 2.3.5 Equality on sum types — Settled

Two values of the same sum type are equal iff they have the same variant tag and their corresponding fields compare equal. Equality on fields uses each field type's equality semantics (structural for transparent and refined types, structural-on-representation for opaque types, recursive for nested sums).

`==` is automatically defined for any sum type whose variants' field types all support equality. A variant carrying a held resource (`Connection[F]`, `Held[T]`) makes the parent sum non-equality-supporting; the compiler errors at any `==` site involving the type.

#### 2.3.6 Pattern testing with `is` — Settled

The `is` operator tests whether an expression matches a pattern, returning `Bool`. It is a general expression form — admissible anywhere a boolean expression is expected, not specific to any one section of the language.

Grammar:

```
expr ::= ...
      |  expr 'is' pattern
```

The pattern follows the grammar of §2.3.4. The result is `Bool`: true if the value matches the pattern, false otherwise.

**Type narrowing.** When `e is p` appears in a position where its truth determines that subsequent expressions are evaluated, the pattern's bindings are introduced into scope and `e`'s type is narrowed accordingly. The positions where narrowing applies:

- *Right operand of `&&`*: `(state is Held(g, r, _, _, _)) && useGuestRoom(g, r)` — in the right operand, `g` and `r` are bound and `state` is narrowed to `Held`.
- *Right operand of `implies`*: `state is Held(_, _, _, _, _) implies state.rsv != ""` — in the consequent, `state` is narrowed to `Held`, so the `rsv` field is accessible.
- *Body of `if`*: `if state is Held(g, r, _, _, _) { ... } else { ... }` — in the then-branch, narrowing applies; in the else-branch, narrowing does not apply (`state` is `BookingState` but known to not be the matched variant — a future enhancement could narrow it to the complementary variants, but v1 does not).
- *Conditional expression analogue*: same rules as `if`.

The narrowing applies only to *positive* assertions of `is`. Negation (`!(x is P)`) does not introduce bindings (the pattern didn't match) and produces no narrowing in the consequent. A `!(x is P)` in the condition of an `if` does narrow the *else* branch (the negative is true in the then-branch, false in the else-branch, which is the positive assertion).

**Binding scope.** Bindings introduced by `is` are scoped to the consequent. After the surrounding boolean expression or if-statement completes, the bindings are out of scope:

```
if x is Some(v) {
  -- v is in scope here, bound to the Some's content
}
-- v is NOT in scope here

let result = (x is Some(v)) && processV(v)
  -- v is in scope in processV(v) only
  -- v is NOT in scope in result's value after the && evaluates
```

**Typing rules.** `e is p` is well-typed when:

- The type of `e` is a pattern-supporting type (sum types, records, tuples, primitives).
- `p` is a valid pattern for the type of `e` per §2.3.4.

Two specific applications of `is` worth noting:

```
-- as a conditional in handler bodies
on confirm() given Clock {
  if state is Held(g, r, dts, rsv, _) {
    let now <- Clock.now()
    state := Confirmed(guest: g, room: r, range: dts, rsv: rsv, at: now)
  }
}

-- in agent invariants (see §2.10.2)
invariant confirmed_has_data:
  state is Confirmed(_, _, _, _, _) implies state.at > epoch
```

The semantics are identical in both contexts; `is` is one operator, not two.

**Or-patterns with `is`.** The pattern after `is` can be an or-pattern; parentheses around the or-pattern are recommended for readability though greedy parsing of the pattern after `is` makes them syntactically optional:

```
if state is (Held(_, _, _, _, _) | Confirmed(_, _, _, _, _)) {
  -- state is narrowed to (Held | Confirmed)
  -- direct field access on state is not generally admissible (the variants' shapes differ);
  -- use a match arm with named bindings if fields are needed
}

if state is (Held(_, r, _, rsv, _) | Confirmed(_, r, _, rsv, _)) {
  -- r: RoomId and rsv: ReservationId are bound and in scope here
  -- the same per-alternative binding consistency rules from §2.3.4 apply
}
```

### 2.4 Nominal records

#### 2.4.1 Declaration — Settled

```
record-type-decl ::= 'type' Name [type-params] '=' '{' field-decl (',' field-decl)* '}'

field-decl ::= Name ':' τ
            |  Name ':' τ 'where' refinement-predicate    -- (see §2.5 for refinement on field types)
```

#### 2.4.2 Construction and access — Open

Construction:

```
let cart = Cart { items: [...], total: ... }
```

All fields must be specified (no defaults at the type level in v1). Type checker verifies field types match.

Field access:

```
cart.total
```

Standard.

Record update — open syntax. Three plausible forms:

1. `{ cart, total: newTotal }` — concise, but punning between rest-spread and full-construction.
2. `cart with { total: newTotal }` — explicit, slightly more verbose.
3. `Cart { ..cart, total: newTotal }` — like Rust's struct update.

Provisional position: option (2) — `with`-based update. Reads clearly, leaves room for future variations (multiple updates, conditional updates).

#### 2.4.3 Equality — Settled

Records are compared field-by-field. Two records of the same nominal type with the same field values compare equal.

### 2.5 Refinement at type declarations

This is the central new commitment (§15 of design notes) and the largest piece of work in the architectural extensions.

#### 2.5.0 Discipline: refinement complements primitive choice — Settled

Refinement is the tool for constraints the primitive type cannot express; it is not a substitute for choosing the right primitive. If a complex refinement is needed to compensate for a loose representation, the representation is likely wrong. The canonical instance: representing money as a fractional number (a `Float`, or a hypothetical exact-decimal type) invites a `Scale(N)` predicate to track precision; representing money as an integer count of the minor currency unit (`{ minorUnits: Int, currency: CurrencyCode }`) makes precision exact by construction and removes the predicate. The discipline guides both language and library design: refinement is the right tool where the primitive cannot reach (`Matches(regex)` on a `String`), and the wrong tool where a sharper primitive would have made the constraint structural.

This shapes the vocabulary in §2.5.2: predicates that compensate for representational looseness (the rejected `Scale(N)` being the leading example) are not in the language; predicates that capture properties the primitive genuinely cannot encode (regex match, numeric range, length, sign, non-emptiness) are.

#### 2.5.1 Declaration grammar — Settled in shape

```
refined-type-decl ::= 'type' Name '=' τ 'where' refinement

refinement ::= predicate
            |  refinement 'and' refinement
            |  '(' refinement ')'

predicate ::= identifier '(' arg-list ')'        -- e.g., Matches("..."), InRange(1, 50)
           |  identifier                          -- e.g., NonNegative, Positive
```

Examples:

```
type VoucherCode = String where Matches("[A-Z0-9]{8}")
type Age = Int where InRange(0, 150)
type Money = { minorUnits: Int where NonNegative, currency: CurrencyCode }
```

The refinement is part of the type's identity. `VoucherCode` and `String` are distinct types.

#### 2.5.2 Predicate vocabulary — Open

The initial vocabulary (small, fixed by the language):

- `Matches(regex: String)` — string matches regex
- `InRange(min: T, max: T)` — numeric value in inclusive range (T : Int | Float; the bounds are literals of the base type, so a `Float` range is `InRange(0.0, 1.0)`; `Instant` takes no refinement predicate)
- `MinLength(n: Int)` — collection or string length ≥ n
- `MaxLength(n: Int)` — collection or string length ≤ n
- `Length(n: Int)` — exact length
- `NonNegative` — numeric ≥ 0
- `Positive` — numeric > 0
- `NonEmpty` — collection or string non-empty

Open: the exact list for v1. Candidates for inclusion in v1: above. Candidates for later: `Sorted`, `Unique`, `OneOf(values...)`, structural patterns like `StartsWith`, `EndsWith`.

Each predicate has:

- *Compile-time semantics*: a procedure the type checker uses to verify refinement preservation under operations.
- *Runtime semantics*: a check applied to candidate values at construction or deserialisation.
- *Schema serialisation*: a translation to OpenAPI/AsyncAPI/JSON Schema fragments.

All three must be implemented; predicates that can't be implemented in all three should not be in the vocabulary.

#### 2.5.3 Generated constructor — Settled

Every refined type declaration generates a constructor function:

```
T.of(v: τ) -> Result[T, ValidationError]
```

Where `τ` is the type's underlying representation. The constructor applies the refinement; success wraps the value as T, failure returns `Err(ValidationError)` describing which predicate failed.

For nested refined types in records, the constructor recursively validates.

The constructor is the only way (from outside the owning context) to produce a value of T. Inside the owning context, the representation is accessible directly, but the type system still treats T as distinct.

#### 2.5.4 Refinement propagation under operations — Open (largest design question)

When an operation is applied to refined values, the type system must decide whether the result preserves the refinement. Three categories:

*Provably preserving.* The compiler can statically derive that the result satisfies the refinement. Examples:

- `(x: Int where NonNegative) + (y: Int where NonNegative)` produces `Int where NonNegative` (sum of non-negatives is non-negative).
- `(s: String where MaxLength(8)).slice(0, 4)` produces `String where MaxLength(4)`, which implies `MaxLength(8)`.
- Pattern matches that constrain a value further preserve any pre-existing refinement.

*Provably non-preserving.* The compiler can derive that the result might not satisfy the refinement. Examples:

- `(x: Int where NonNegative) - (y: Int where NonNegative)` is `Int` (not `Int where NonNegative` — subtraction may produce negative).
- `(s: String where Length(8)) ++ "x"` is `String` (length now 9).

*Unprovable.* The compiler cannot statically derive whether preservation holds. In this case the result is the unrefined type; the user must re-construct to recover the refinement.

Open: the precise set of preservation rules. This needs careful design — it's where most of the type system's subtlety lives. Probably defined as a built-in table mapping (predicate, operation) → preservation rule, with fallback to "result is unrefined."

#### 2.5.5 Composition of refinements — Open

When a refinement is multiple predicates joined by `and`:

```
type ProductCode = String where Matches("[A-Z]{3}") and MaxLength(8)
```

The compiler treats this as a single refinement that must satisfy both predicates. Construction validates both. Schema generation emits both as constraints.

Open: how predicate composition interacts with propagation (§2.5.4). When an operation preserves one predicate but not the other, is the result refined by the preserved predicate only, or fully unrefined? Probably the former, but the rule needs to be made precise.

#### 2.5.6 Refinement on refined types — Open

Can a refined type itself be refined?

```
type PostalCode = String where Matches("[A-Z]{2}[0-9]{2}")
type UKPostalCode = PostalCode where MatchesPrefix("UK")
```

This is a useful pattern (extending a base refined type with further constraints). The refinement on `UKPostalCode` composes with the refinement inherited from `PostalCode`.

Provisional position: yes, refined types can be refined. Operations on `UKPostalCode` first satisfy `PostalCode`'s refinements, then `UKPostalCode`'s additional ones.

Open: the syntax and the propagation rules for layered refinement.

#### 2.5.7 Boundary validation — Settled in shape

Validation runs at **wire-crossing boundaries** where a value transitions from untrusted external representation to typed in-process value. The validation is the refined type's constructor (`T.of(v)`) applied to the deserialised representation; success produces a validated value, failure produces a structured error appropriate to the boundary.

The enumerated boundaries where validation runs:

- *HTTP service ingress*: request body, headers, URL parameters, query string. Failure → HTTP 400 with field-level error detail.
- *Queue message receive*: payload deserialisation. Failure → dead-letter per the platform's policy.
- *Event subscriber receive*: payload deserialisation. **Correction (#973):** as shipped, failure is a `400` at the receiving Workers route, which the fan-out DO's per-subscriber catch turns into a logged delivery failure to that one subscriber — there is no dead-letter policy for the Events track's fanout-DO substrate (`design/tracks/events.md` §3.1, §6). The dead-letter outcome below is real only for a native `on queue` consumer, which does sit on an actual Cloudflare Queue.
- *Cross-Worker call deserialisation*: arguments arriving from another Worker (via Service Binding or RPC). Failure → fault at the receiving handler.
- *Storage rehydration*: agent state loaded from durable storage at startup or recovery. Failure → fault at agent initialisation (schema corruption is not silently recoverable).
- *HTTP service egress* (responses): validation is the *sender's* responsibility — values produced inside the handler are already typed and need not be re-validated, but if a response is constructed from untyped sources (rare), validation should be applied.

The boundaries where validation does **not** run:

- *In-process cross-context calls* (between contexts deployed in the same Worker, or invoked via the runtime's intra-process dispatch). The values are already typed; no re-validation. The compiler emits a direct call.
- *Cross-agent calls within the same Worker* (between agents in the same DO, or between DOs in the same Worker when the binding is in-process).
- *Internal helpers and pure functions*. Their typed parameters and return values flow without validation.
- *Closure invocations* (calling a captured closure with typed arguments).

The distinction is the trust boundary, not the call site. A value that has been a `VoucherCode` in-process is a `VoucherCode` everywhere it flows in-process. A value crossing from a wire (HTTP, queue, RPC, storage) is reconstructed from bytes and must be validated.

Open: the precise error format for each boundary type; how field-level errors are aggregated for records with multiple invalid fields; the policy for partial validation (when one field fails, do other fields' errors also get reported, or short-circuit?); whether wire-crossing detection is determined by the build (each context's deployment unit is known at build time) or by runtime annotation.

#### 2.5.8 Serialisability of refined types — Settled in shape

A refined type is **serialisable** if its underlying representation is serialisable and its predicate can be re-applied to the deserialised value. The compiler tracks serialisability as a static property of types.

Serialisability rules:

- Primitive types (`Int`, `Float`, `String`, `Bool`, `Bytes`, `Instant`, `Duration`, `Unit`) are serialisable.
- Refined primitive types are serialisable; the predicate is re-applied on deserialisation.
- Closed sum types are serialisable when every variant's payload is serialisable.
- Nominal record types are serialisable when every field's type is serialisable.
- Opaque types are serialisable when their representation is serialisable.
- Function types and closures over arbitrary captures are *not* generally serialisable.
- Held resources (`Connection[F]`, `Held[T]`) are *not* serialisable.
- A `Stream[T]` (v0.100, real-time track slice 0) is *not* serialisable — a live, pull-shaped value-over-time source, built and consumed in place, never persisted or sent across a boundary (the same non-storable/non-boundary discipline `Query`/`Effect` obey).
- Storage type references (`Ref[A]`, `Cell[T]`, `Map[K, V]`, etc.) are serialisable in the sense that the *reference* serialises (an addressable identity), but the referenced state is not transported.
- A closure is **statically serialisable** when its captured environment consists entirely of serialisable values and its body is a single cross-agent or capability call. This is a restricted form sufficient for compensation actions and capability operation refinements; arbitrary closure serialisation is out of scope.

The serialisability property has practical consequences across the language:

- *Compensation closures in `Sagas.compensate(...)` calls* should be statically serialisable when the bound `Sagas` provider is durable. While the in-memory `Sagas` provider keeps closures in handler-local state (non-durable), the durable provider serialises them as call descriptors (agent ref, method name, captured arguments). The compiler should check serialisability of compensation closures and warn (or error) when they capture non-serialisable values in handlers that bind a durable Sagas provider, because a later refactor that switches providers would otherwise fail at runtime.
- *Event payloads* must be serialisable (they cross the event bus's wire).
- *Cross-Worker capability operation arguments and results* must be serialisable.
- *Durable storage values* (any value written to `Cell`, `Map`, `Set`, `Log`, `Queue`) must be serialisable.

The serialisability check is a separate type-system pass that runs after refinement checking and produces its own diagnostics (e.g., "closure captures `Connection[F]`, which is not serialisable; this closure cannot be used in a context that requires durable representation").

Open: the precise interaction with the capability operation refinement vocabulary (closures used as operation arguments inherit the operation's serialisability requirements); whether serialisability can be parametric (e.g., `Map[K, V where Serialisable]` as a type-level constraint); diagnostics for serialisability failures.

#### 2.5.9 Schema generation — Open

The refinement vocabulary serialises to external schemas:

- `Matches(R)` → JSON Schema `pattern: R`
- `InRange(min, max)` → `minimum: min, maximum: max`
- `MinLength(n)` → `minLength: n`
- `MaxLength(n)` → `maxLength: n`
- `Length(n)` → `minLength: n, maxLength: n`
- `NonNegative` → `minimum: 0`
- `Positive` → `exclusiveMinimum: 0`
- `NonEmpty` → `minLength: 1` for strings; `minItems: 1` for arrays

Open: the precise mappings for OpenAPI vs AsyncAPI vs raw JSON Schema; how nested refined types serialise; how recursive types with refined components are handled.

### 2.6 Capability interfaces and `given` clauses

#### 2.6.1 Declaration — Settled in shape

```
capability-decl ::= 'capability' Name '{' op-decl+ '}'

op-decl ::= Name '(' param-list ')' '->' return-type [refinement-clause]
```

Examples:

```
capability Inventories {
  reserve(sku: Sku, qty: Quantity, orderId: OrderId) -> Effect[ReserveOutcome]
  release(sku: Sku, rid: ReservationId) -> Effect[Unit]
}

capability Vouchers {
  lookup(code: VoucherCode) -> Effect[Option[Voucher]]
    where ReadOnly and Idempotent
  redeem(code: VoucherCode, customer: CustomerId)
    -> Effect[Result[DiscountPercent, RedemptionError]]
    where Idempotent on (code, customer)
}
```

#### 2.6.2 `given` clauses — Settled in shape

Functions and handlers declare which capabilities they use:

```
fn helper(x: Int) -> Effect[Money] given Payments, Clock { ... }

on place(u: UserId, c: Cart) -> Result[Receipt, OrderError]
    given Inventories, Payments, Fulfilments { ... }
```

The `given` clause is part of the function's type. A function with `given C` can only be called from contexts where `C` is itself available (either declared in scope, provided locally, or threaded through the caller's own `given`).

#### 2.6.3 Resolution — Open

At link time, every `given C` site must resolve to a `provides C` declaration:

```
provides-decl ::= 'provides' Name '{' op-impl+ '}'
```

Open questions:

- *Multiple providers in scope.* What happens if two contexts in scope both `provides C`? Probably: compile error (no automatic disambiguation); the consumer must explicitly select.
- *Provider declaration syntax.* The exact form for declaring an implementation. Probably maps each capability operation to a function or method call.
- *Inheritance.* If context A consumes B which provides C, does A see C? Probably yes (transitively visible through the `consumes` chain).

#### 2.6.4 Inference of `given` — Open

For top-level handler signatures, `given` clauses are explicit. For internal helper functions, the question is whether `given` is inferred from usage or must be declared.

Provisional position: explicit at handler boundaries, inferable on internal helpers (the checker propagates `given` requirements up the call chain and adds them to the helper's inferred type). This matches the language's annotation policy of "required at contract boundaries, inferred internally" (§15 of design notes).

Open: whether *all* helpers can have inferred `given`, or only those marked somehow, and how this interacts with capability operation refinements (§2.6.5).

#### 2.6.5 Capability operation refinements — Open

Capability operations may carry refinements that the implementation must satisfy and that consumers can rely on:

- `ReadOnly` — the operation does not modify storage
- `Idempotent` — running the operation more than once has the same effect as once
- `Idempotent on (k₁, ..., kₙ)` — idempotent with a specified dedup key

These are checked at the implementation side (the compiler verifies `provides` implementations satisfy the operation's refinements) and propagated to consumers as guarantees.

Open: the precise vocabulary of capability operation refinements; how `ReadOnly` is checked (probably by examining the implementation for writes to storage primitives); how `Idempotent on (...)` interacts with the implementation's own use of the `Idempotency` capability.

### 2.7 Storage type kinds

#### 2.7.1 The built-in kinds — Settled

The storage type kinds are language built-ins:

- `Cell[T]` — single-value cell, with `:=` and `.update(fn)` operations
- `Map[K, V]` — keyed map, with `.put`, `.get`, `.update`, `.upsert`, `.remove`, query operations
- `Set[T]` — set of values, with idempotent `.add` and `.remove`
- `Log[T]` — append-only log, with non-idempotent `.append` and time-window query operations
- `Queue[T]` — FIFO queue for delayed work (Open: details TBD)
- `Cache[K, V]` — bounded cache with eviction
- `Ref[A]` — reference to another agent
- `Connection[F]` — a held WebSocket-like connection parameterised by what can be sent through it
- `Held[T]` — a held resource (parent kind of `Connection`)

These are not user-definable type constructors. They have built-in operations the compiler knows about.

**All storage operations are Effect-typed** (returning `Effect[T]` for some T), reflecting that storage access is a real effect on the system state. Consuming a storage operation in a handler requires `<-` to await. This makes storage cost visible at the call site, mirrors the discipline applied to cross-context calls, and remains honest across compilation targets where storage may be genuinely async.

The single ergonomic exception is `Cell[T]`, which provides syntactic sugar for the most common reads and writes:

- *Implicit deref on read.* When a `Cell[T]` appears in a value position (e.g., `if available < qty`, `let x = balance + n`), the compiler inserts a `<- cell.read()` operation. The expression `available` is sugar for `<- available.read()`.
- *Assignment as sugar.* `cell := expr` desugars to `<- cell.write(expr)`. The statement form is sugar for the Effect-typed write.

Both sugars preserve the Effect-typed underlying operations; they only hide the await syntax for the most common idioms. Other Cell operations (`cell.update(fn)`, `cell.swap(v)` if added) require explicit `<-`. All other storage types (`Map`, `Set`, `Log`, `Queue`, `Cache`) have **no** sugar — every operation site is an explicit `<-`.

Implication: Cell is the right primitive for single-value state where the read/write idiom dominates. For storage where operations are heavier or more semantically interesting (map updates, log appends, set membership checks), the explicit await is appropriate and the cost is visible.

#### 2.7.2 Refined element types — Settled in shape

When a storage type's element parameter is refined, the refinement propagates through operations:

```
store available: Cell[Int where NonNegative]
```

Writes to this cell (`available := newValue` — desugared to `<- available.write(newValue)`) require `newValue` to be `Int where NonNegative`. The compiler enforces this at the write site or, when not statically provable, requires explicit construction via the refined type's constructor.

On rehydration from durable storage, values are validated against the refined type's predicate.

#### 2.7.3 Operations and their types — Open

Precise type signatures for each operation (returning `Effect[T]` throughout). Mostly settled in §10 of design notes; this section formalises them. The forms are illustrative **except for the `Cell` operations, which are normative as of v0.98 (ADR 0125)** — see the note below the block:

```
Cell[T].read()       : Effect[T]                          -- desugaring target; not a surface method
Cell[T].write(v: T)  : Effect[Unit]                       -- desugaring target; not a surface method
Cell[T].update(f)    : Effect[Unit]                       -- f: (T) -> T  (the one method-shaped Cell op)

Map[K, V].put(k, v)        : Effect[Unit]
Map[K, V].get(k)           : Effect[Option[V]]
Map[K, V].update(k, f)     : Effect[Unit]                 -- f: (V) -> V
Map[K, V].upsert(k, init, f) : Effect[Unit]               -- f: (V) -> V; init: () -> V
Map[K, V].remove(k)        : Effect[Unit]
Map[K, V].contains(k)      : Effect[Bool]
Map[K, V].keys             : Query[K]                    -- v0.158, ADR 0184
Map[K, V].values           : Query[V]                    -- v0.158, ADR 0184
Map[K, V].entries          : Query[MapEntry[K, V]]       -- v0.158, ADR 0184 (nominal entry record, not a tuple)

Set[T].add(x)              : Effect[Unit]
Set[T].remove(x)           : Effect[Unit]
Set[T].contains(x)         : Effect[Bool]
Set[T].isEmpty             : Effect[Bool]
Set[T].size                : Effect[Int]
Set[T].values              : Query[T]

Log[T].append(x)           : Effect[Unit]
Log[T].size                : Effect[Int]
Log[T] query builders      : Query[T] (lazy; terminate to execute)
```

Query terminals (`collect`, `first`, `count`, etc.) all return `Effect[T]`. Builders (`filter`, `map`, etc.) are pure, returning `Query[T]`.

**Normative — `Cell` operations (v0.98, ADR 0125).** A cell exposes exactly one method-shaped operation, `update(f: (T) -> T) : Effect[()]`, a read-modify-write that makes the prior-value dependency explicit (the form a self-referencing `:=` is steered toward). `read` and `write` are *not* callable surface methods: a cell is read by its bare name (implicit-deref sugar, §2.7.2) and written with `:=`. They appear above only as the desugaring targets those sugars name. The combiner `f` is a pure `(T) -> T`; an effectful body (including a bare read of another cell, itself effectful sugar) is rejected.

**Normative — `Map` key-aware queries (v0.158, ADR 0184).** A `store Map[K, V]` field exposes three key-aware lazy queries: `map.keys : Query[K]`, `map.values : Query[V]`, and `map.entries : Query[MapEntry[K, V]]`. `MapEntry[K, V]` is a compiler-known nominal record `{ key: K, value: V }` — bynk has no tuple/pair type (ADR 0120), so an entry is *named*, read with `.key`/`.value`; the whole single-argument query vocabulary applies to `.entries` unchanged. `MapEntry` is a generic-record instantiation and so **non-boundary** (ADR 0183): a read handler projects each entry into a named boundary type (`items.entries.map((e) => Row { … })`) before its terminal, which is what lets the stored value drop the denormalised copy of its own key. These three are paren-less query accessors on the *storage* map, distinct from the eager in-memory `Map` value methods `.keys()`/`.values()` (§ collections, ADR 0035).

The remaining operations in this block stay illustrative — to be filled in incrementally as edge cases surface.

#### 2.7.4 Usage restrictions — Settled in shape

Storage types are valid only in specific positions:

- `Cell`, `Map`, `Set`, `Log`, `Queue`, `Cache` — only inside agent declarations, as `store` fields. The `store` keyword is a required prefix on agent state field declarations.
- `Ref[A]` — anywhere a value is needed (not a storage type itself, but a reference *to* an agent that owns storage).
- `Connection[F]`, `Held[T]` — only in specific positions (agent state, handler parameters, specific operation arguments); see §2.9.

Initial values for storage fields use literal forms appropriate to the kind:

- `Cell[T] = expr` — initial value of T.
- `Map[K, V] = {}` — empty map literal.
- `Set[T] = {}` — empty set literal (context-disambiguated from empty map by the type).
- `Log[T] = []` — empty log literal.
- `Queue[T] = []` — empty queue literal.

Open: the precise rules for initial values that themselves require Effect-typed construction (e.g., a Cell initialised to a value derived from another storage read); diagnostics for misuse.

#### 2.7.5 In-memory storage — Deliberately deferred

No in-memory (non-persistent, sync-access) storage type is in v1. The agent's `store` fields are durable; local `let` bindings within handlers are the only sync-access state. A future `Local[T]` or `Transient[T]` kind for in-memory caches that don't persist could be added if a real need emerges, but is deliberately not part of v1 to keep the storage model uniform.

### 2.7.6 Built-in value-type vocabulary — Settled in shape

A small set of value types is built into the language with known operations the compiler is aware of.

*`Option[T]`* — a closed sum `None | Some(T)` with a small monadic API:

```
Option[T].isSome      : Bool
Option[T].isNone      : Bool
Option[T].map(f)      : Option[U]                -- f: (T) -> U
Option[T].flatMap(f)  : Option[U]                -- f: (T) -> Option[U]
Option[T].filter(p)   : Option[T]                -- p: (T) -> Bool
Option[T].getOrElse(d): T                        -- d: () -> T (lazy default)
Option[T].toResult(e) : Result[T, E]             -- e: () -> E
```

*`Result[T, E]`* — a closed sum `Ok(T) | Err(E)` with the `?` propagator:

```
Result[T, E].isOk          : Bool
Result[T, E].isErr         : Bool
Result[T, E].map(f)        : Result[U, E]            -- f: (T) -> U
Result[T, E].mapErr(f)     : Result[T, F]            -- f: (E) -> F
Result[T, E].flatMap(f)    : Result[U, E]            -- f: (T) -> Result[U, E]
Result[T, E].flatMapErr(f) : Result[T, F]            -- f: (E) -> Result[T, F]
```

The `?` operator (§2.8.3) propagates `Err` from `Result[T, E]` in any function returning `Result[U, E]` (with the same `E`).

*`List[T]`* — an ordered, immutable in-memory sequence with the query and effectful-iteration vocabulary documented in §11 of design notes. Builders return `Query[T]` (lazy); terminals execute. Effectful iteration (`traverse`, `parTraverse`, `traverseAll`, `parTraverseAll`) is in scope.

*Tuples* — **not part of the language** (#1530). Bynk stays nominal: [ADR 0120](decisions/0120-join-group-combiner-form.md) D1 introduces no pair or tuple type, and a general n-ary tuple is a named deferral, not a settled built-in. The place tuples were meant for, the query algebra's join and group results, takes an `into:` combiner that names the result instead (ADR 0120 D2/D3). `(1, "a")`, `(Int, String)` and `let (a, b) = …` are parse errors.

*Primitive value types* (per §1.1) — `Int`, `Float`, `String`, `Bool`, `Bytes`, `Instant`, `Duration`, `Unit` — carry standard operations: arithmetic on numerics; comparison on totally-ordered types; string operations (length, slice, concatenation `++`, etc.); boolean logic (`&&`, `||`, `!`); temporal arithmetic where `Instant + Duration = Instant`, `Instant - Duration = Instant`, `Instant - Instant = Duration`, `Duration + Duration = Duration`, and `Duration * Int = Duration`.

`Duration` literals use the `N.unit` form on integer literals: `5.minutes`, `24.hours`, `7.days`, `100.milliseconds`, `2.weeks`. Each desugars to a `Duration` value with the integer count converted to milliseconds. The recognised units are `milliseconds`, `seconds`, `minutes`, `hours`, `days`, `weeks` — calendrical units like `months` and `years` are *not* available on `Duration` because their length varies; those belong to `bynk.time` calendrical arithmetic.

Comparison between a refined type and its underlying representation works on the representation's equality/ordering: `(x: Int where InRange(0, 100)) < (y: Int)` compiles and behaves like `Int` comparison. Refinement is a constraint on values, not a different comparable kind.

Open: the precise method list for `Option`, `Result`, `List` (probably more than sketched here).

### 2.8 Effects and the await operator

#### 2.8.1 The `Effect` type — Settled in shape

`Effect[T]` marks an asynchronous or effectful computation. A function returning `Effect[T]` is effectful; calling it requires an explicit await.

The `Effect` type is monadic in the technical sense but the language exposes it through specific operators rather than as a general monad.

Standard sources of `Effect[T]` in Bynk programs:

- *Storage operations.* All operations on `Cell`, `Map`, `Set`, `Log`, `Queue`, `Cache` are Effect-typed (§2.7.1). Cell reads and `:=` writes are sugared (the await is compiler-inserted), but underlying operations are Effect.
- *Cross-agent calls.* Invoking a handler on another agent via `Ref[A]` or capability-resolved agent reference returns `Effect[T]`.
- *Capability operations.* Operations declared in `capability` interfaces return `Effect[T]` for their result types (§2.6.1).
- *Built-in platform capabilities.* `Clock.now()`, `Random.next()`, `Http.fetch(...)`, etc. — all platform capabilities return `Effect[T]`.

Functions composed of pure operations (no storage access, no `given` clauses, no calls to effectful operations) need not return `Effect[T]`. The type system infers effectfulness from the function body; explicit `Effect[T]` in a signature is allowed where the contract should be committed.

#### 2.8.2 The `<-` await operator — Settled in shape

`let x <- expr` is the await operator. Requires `expr : Effect[T]` for some T, and binds `x : T` in the rest of the scope.

```
on place(u: UserId, c: Cart) -> Result[Receipt, OrderError] given Inventories, Payments {
  let reservations <- reserveAll(c.items)?
  let authId <- Payments.authorise(c.total, u)?
  ...
}
```

A function containing `<-` must return `Effect[U]` for some U (or a wrapper like `Effect[Result[T, E]]`). The `Effect` is "infectious" — once awaited, it threads through the calling function's return type.

#### 2.8.3 Interaction with `Result` and `?` — Settled in shape

`Effect[Result[T, E]]` is the universal shape of cross-context calls in Bynk. The volume of this composition warrants specific language support beyond what either `Effect[T]` or `Result[T, E]` provides individually.

**Composed methods.** In addition to Effect's standard `.map` and `.flatMap`, the following methods are available directly on values of type `Effect[Result[T, E]]`:

```
mapOk(f: (T) -> U)                           : Effect[Result[U, E]]
mapErr(f: (E) -> F)                          : Effect[Result[T, F]]
flatMapOk(f: (T) -> Effect[Result[U, E]])    : Effect[Result[U, E]]
flatMapErr(f: (E) -> Effect[Result[T, F]])   : Effect[Result[T, F]]
```

Each desugars to a predictable composition of Effect's and Result's operations:

```
e.mapOk(f)      ≡  e.map(r => r.map(f))
e.mapErr(f)     ≡  e.map(r => r.mapErr(f))
e.flatMapOk(f)  ≡  e.flatMap(r => match r {
                       Ok(v)    => f(v)
                       Err(err) => Effect.pure(Err(err))
                   })
e.flatMapErr(f) ≡  e.flatMap(r => match r {
                       Ok(v)    => Effect.pure(Ok(v))
                       Err(err) => f(err)
                   })
```

The naming is verb-first throughout (`map`, `mapErr`, `mapOk`, `flatMapOk`, `flatMapErr`), matching the convention used on `Result` directly and consistent with `map`/`flatMap` on `Effect`. The compiler synthesises these methods for any concrete `Effect[Result[T, E]]`; they're not separate definitions in user code or the standard library. The desugaring is the semantics — the methods are syntactic sugar.

**Why these specific methods.** `mapOk` and `mapErr` cover the two transformation directions on the success/failure split. `flatMapOk` covers the most common chaining case (another effectful fallible operation continuing on success). `flatMapErr` covers error recovery (an effectful operation attempting to recover, e.g., consulting a fallback source or applying a default for a specific error variant). The full set is small, symmetric, and exhausts the common patterns.

**Why named methods rather than auto-lifting.** The general rule "methods of `T` are accessible on `Effect[T]` via implicit `.map` lifting" was considered and rejected. The problem is `.map` ambiguity: both `Effect` and `Result` have a `.map`, and on `Effect[Result[T, E]]` the question "which `.map`?" has no good answer. Naming the lifted operations explicitly (`mapOk` for "map the success value", `mapErr` for "map the error type") removes the ambiguity. `.map` on `Effect[Result[T, E]]` is unambiguously Effect's `.map`, operating on the whole `Result`.

The user can always fall back to `e.map(r => r.someResultMethod(...))` when a Result method that isn't lifted is needed. The lifted set covers the common cases; the explicit map covers everything else.

**No general lifting for other Effect-of-X compositions.** `Effect[Option[T]]`, `Effect[List[T]]`, and other shapes have no compiler-synthesised methods in v1. Users write `e.map(o => o.method())` explicitly. This keeps the surface narrow and avoids surprise.

**Combination with `<-` and `?`.** The combination flows cleanly:

```
let authId <- Payments.authorise(amount, user).mapErr(toBookingError)?
```

Parsing and typing:

- `Payments.authorise(amount, user)` : `Effect[Result[AuthId, PaymentError]]`
- `.mapErr(toBookingError)` : `Effect[Result[AuthId, BookingError]]`
- `<-` peels off `Effect` : `Result[AuthId, BookingError]`
- `?` propagates `Err` in any function returning `Result[_, BookingError]` : `AuthId`

The `?` operator is postfix on the value produced by `<-`. Parsing precedence places `?` higher than `<-`: `let x <- expr?` parses as `let x <- (expr?)`, where `expr?` is the postfix application of `?`. Since `?` requires a `Result`-typed receiver, this parsing is well-defined when `expr : Effect[Result[T, E]]` — `expr?` is a type error directly (Effect isn't Result), so the parser resolves to `<- (expr)?` and the typing flows. The grammar admits both readings; the type system disambiguates by requiring `?` on Result and `<-` on Effect.

Open: whether the parsing-level ambiguity should be made explicit in the grammar (forcing a single reading) or left to the type system (current approach).

#### 2.8.4 Effect inference — Settled

Effects are **declared on named declarations and inferred only on lambdas** (#1529):

- A `fn`, a handler, and a capability or provider operation state their return type, `Effect[T]` included; a `fn` without `->` is a parse error. Their capabilities are declared by `given` and checked against the body, never inferred from it.
- An unannotated lambda's effect is read off its body: a lambda whose body binds with `<-` or calls a capability is effectful, and its type is `(…) -> Effect[T]`. Where an expected function type is present, the lambda is checked against it instead.

This is design notes §15's "declared, not inferred" for everything with a name. The lambda carve-out does not breach the stability firewall ([`bynk-greenfield-compiler.md`](bynk-greenfield-compiler.md) R3.14): a lambda has no signature that any other declaration depends on, so editing its body can change only the type of the expression it sits in, never a named declaration's signature.

### 2.9 Held resources

Held resources are the carrier for runtime-managed values whose lifetimes need explicit management: WebSocket connections being the canonical instance. The design is **API-discipline-driven linearity** — the language tracks linearity through a fixed vocabulary of operations the runtime exposes on held types, rather than through general linear-types machinery.

> **Status (v0.102, real-time track slice 2, ADR 0130):** the `Connection[F]` type, its `send`/`close` operations, the storage-admission rules (§2.9.3), and the linearity-check pass (§3 step 11) — the ownership states, mandatory disposal, branch unification — are **built**. §2.9.7 is settled for the **within-handler** subset (below). The held-aware iteration borrow surface (`forEach`/`parTraverse` over connections), record-of-held (§2.9.9), and cross-context fault propagation are later slices / deferred.
>
> **Status update (v0.103–v0.107, slices 3a–4, ADRs 0131–0135 — the track is COMPLETE):** the `from WebSocket` protocol is built on **both targets**, a stored connection **survives hibernation** on Workers, the channel is **bidirectional**, and a message **fans out** to every connection in a room. Slice 3a (ADR 0131) shipped the protocol surface, the edge-auth-before-accept checker rules, and the bundle vertical against `TestConnection`. Slice 3b-i (ADR 0132) shipped the **Cloudflare Workers wire path**: the Worker authenticates the actor at the edge (the Bearer token read from `Sec-WebSocket-Protocol`, fail-closed) and forwards the upgrade to the hosting Durable Object, which accepts the socket and runs the `on open` body. Slice 3b-ii (ADR 0133) shipped the **hibernation re-association** (§2.9.6): the DO accepts via `state.acceptWebSocket(server, [connId])` (hibernatable), and a held `store Map[K, Connection]` persists `Record<K, connId>` — the stored value is the connId, and each `Connection` re-resolves to its live socket via `getWebSockets(connId)`, so a stored connection survives eviction; `remove` resolves-closes-deletes. Slice 3b-iii (ADR 0134) shipped the **inbound/close half**: `on message(frame)` and `on close` service handlers (like `on open`) run in the hosting DO on `webSocketMessage`/`webSocketClose`, decoding the frame against `in:` fail-closed (a malformed frame closes the socket, never dispatched) and recovering the sender identity + route args from the socket attachment (set at `on open` — authenticated once); the firing `connection` is a borrowed held binding. Slice 4 (ADR 0135) shipped the **closure**: `parTraverse` — the parallel broadcast over a held `store Map[K, Connection]` (each connection borrowed, sent concurrently; the held-aware iteration borrow surface) — and the **§20 chat-room running end-to-end** on bundle (a message fans out to every connection in the room). **The real-time / WebSocket track is complete.** Held-resource work outside this track (§2.9.7 cross-context fault propagation, §2.9.9 record-of-held / user-defined borrows) stays as named, deferred follow-ons.

#### 2.9.1 The kind — Settled in shape

`Held[T]` is a kind, with concrete instances supplied by the language and platform:

- `Connection[F]` — a typed handle to a WebSocket connection. `F` is the type of frames the server can send through the connection (the channel is typed; sending a wrong-shaped frame is a compile error at the `.send` site).
- Future `Held` instances may be added as the platform's capability surface grows (file handles, long-running database connections, GPU contexts) but each addition is a language-level commitment.

Held types are *not* user-definable. The kind is closed at v1; new held types require language work.

Defining properties:

- *Origin is restricted.* Held values come only from runtime-provided sources — capability operations producing fresh held values, or handler parameters supplied by the framework. There is no public constructor.
- *Sharing is bounded.* A held value has at most one owner at any time. Duplication (two bindings holding the same value) is a compile error.
- *Disposal is mandatory.* A held value that's an owner at scope exit must have been transferred (to storage, to another owner, to a consuming operation) before exit. Letting it go out of scope is a compile error.

#### 2.9.2 The state model — Settled in shape

The compiler tracks each held variable through a small state machine:

```
[fresh]  →  [owned]  →  [borrowed]  →  [owned]  →  [consumed]
                ↓
            [stored]
```

- **fresh**: a held value just produced by the runtime; not yet bound to a variable.
- **owned**: bound to a variable; the variable owns the value; can be operated on directly.
- **borrowed**: temporarily lent to an operation (typically a storage primitive's closure-receiving method); the borrow has a lexical scope; cannot be consumed or transferred during the borrow; reverts to **owned** when the borrow's scope ends.
- **stored**: ownership transferred into a storage primitive (`Cell`, `Map`, `Option[Held[T]]` in a `Cell`); the variable is no longer the owner.
- **consumed**: a consuming operation has used the value; the variable can no longer be referenced.

Transitions are determined by operation classifications:

- *Receive operations* (capability operations producing held values, handler parameters of held type) produce owned bindings.
- *Borrowing operations* lend ownership for a lexical scope. The classic borrowing operation is a storage primitive's higher-order method like `map.forEach(fn)` or `map.update(k, fn)`; `fn` receives a borrowed reference, valid only inside `fn`'s body.
- *Consuming operations* end the value's lifetime. The canonical consume operations are `close(c)`, storage primitive `put(k, v)` (transfers ownership to storage), and storage primitive `take(k)` (transfers ownership out of storage to the caller).
- *Non-consuming operations* on owned bindings are operations the runtime classifies as borrow-and-return. The classic example is `c.send(msg)` on a Connection: the send doesn't end the connection's life; the binding stays owned after.

#### 2.9.3 Storage of held resources — Settled in shape

Held values can be stored in:

- `Cell[Option[Held[T]]]` — a single optional held value. Common for "this agent has a primary connection" patterns.
- `Map[K, Held[T]]` — keyed collection. The common pattern for "this agent manages many connections."

Held values **cannot** be stored in:

- `Set[Held[T]]` — Set semantics require equality on members; held values have identity but not value-equality.
- `Log[Held[T]]` — retaining all historical held resources indefinitely doesn't match any real use case.
- `Cache[K, Held[T]]` — eviction of a held resource without explicit close is unsafe; cache semantics conflict with mandatory disposal.

Storage operations on held-bearing collections:

- `put(k, v)` — moves ownership of `v` into the collection. After, `v` is consumed at the call site (the binding is no longer usable).
- `take(k) -> Option[Held[T]]` — removes the value at `k` from the collection and transfers ownership to the caller. The caller now owns the value (if present) and must dispose of it.
- `remove(k)` — like `take(k)` but discards the returned value; the runtime closes the held value implicitly. Convenience for "remove and close" patterns.
- `forEach(fn: (K, &Held[T]) -> Effect[Unit])` — iterates with borrowed references. Each callback receives a borrowed reference; the borrow ends when the callback returns. Cannot consume or transfer during the iteration.
- `update(k, fn: (&Held[T]) -> Effect[T])` — applies a function to the value at `k` with a borrowed reference; the borrow's scope is the function call.
- Query operations (filter, map, etc., as part of the query algebra) produce queries over *borrowed* views; terminals like `parTraverse` invoke their closures with borrowed references.

The borrow type `&Held[T]` (notation: not user-written; the type system infers it at borrowing operation sites) admits the non-consuming operations on `Held[T]` but rejects the consuming ones. Attempting to call `close` on a borrowed reference is a compile error pointing at the operation site.

#### 2.9.4 Handler parameters and return types — Settled in shape

A handler may receive a held value as a parameter when the framework supplies one (the canonical case is the WebSocket `open` handler receiving a fresh `Connection[F]`). The parameter is an *owned binding* inside the handler. The handler must dispose of it before returning:

- store it in agent state (`connections.put(userId, conn)`),
- consume it (`close(conn)`),
- pass it to another function that consumes it (transferring ownership).

A handler can return a held value, but only in tightly restricted cases (probably v1 disallows this entirely — held returns from handlers complicate the storage model significantly). The provisional rule: handlers do not return held values; held values flow into agents via parameters and out via the runtime's lifecycle machinery.

Internal helper functions can take and return held values, with the standard ownership semantics: parameters are owned (and must be consumed or returned), returns are owned by the caller.

#### 2.9.5 Branching and loops — Settled in shape

**Branch unification.** All branches of a conditional must leave each held binding in a compatible state.

```
if cond {
  close(conn)        -- conn: consumed
} else {
  store.put(k, conn) -- conn: consumed (transferred to storage)
}
-- after: conn is consumed in both branches; OK.
```

But:

```
if cond {
  close(conn)        -- conn: consumed
}
-- else branch implicit: conn is still owned
-- after: conn is consumed-or-owned, inconsistent → compile error
```

The compiler errors with a diagnostic pointing at the branch that doesn't consume, and suggests either adding an else-branch consume or making both branches consistent.

**Guards.** A `match` arm's `if` guard is a **borrow scope** over every held binding it can see: the arm's own pattern bindings and any held binding in scope from outside (#1769). A guard runs before its arm is chosen, so it may run for an arm that is then not taken; a consume inside it would happen on a path where the value is still owned. A guard may therefore use a held value only in a non-consuming way. A consuming use (a transfer, `close`) is `bynk.held.consume_on_borrow`, and naming a value already consumed is `bynk.held.use_after_consume`. The borrow ends with the guard, so the guard changes no ownership state: the arm body, later arms, and the unification after the `match` all see the state from before it.

```
match slot {
  Some(conn) if ready => conn.close()        -- OK: the guard names no held value
  Some(conn) if Some(conn) is Some(_) => …   -- error: `conn` is transferred inside a guard
  …
}
```

Two other rules were rejected. Counting a guard's use as a transfer on the path where its arm is taken is unsound, because the guard has already run on the paths where the arm is not. Forbidding held values in guards outright is the borrow rule without the non-consuming uses. It would need a new diagnostic and would reject a future `Bool`-returning non-consuming operation for no reason.

**Loops.** A held value cannot meaningfully cross loop iteration boundaries — once consumed, it can't be re-consumed; if borrowed inside, the borrow must end before the next iteration. Two valid patterns:

- *Per-iteration acquisition.* Each iteration acquires a held value via storage iteration (borrows), operates, and lets the borrow end naturally at the iteration boundary. This is the canonical pattern: `connections.values.parTraverse(c => c.send(msg))`.
- *Acquire-once, consume-inside.* A held value acquired before a loop must be consumed within the loop body (not on every iteration; in exactly one iteration). This is rare; most cases reduce to the per-iteration pattern.

A held value owned before a loop, used inside without consumption or borrowing, is a compile error: the loop would keep referencing a value past iteration boundaries with no defined semantics.

#### 2.9.6 Hibernation and the platform — Settled in shape

The Cloudflare Durable Objects platform provides hibernatable WebSockets: a `Connection[F]` stored in agent state survives the agent's hibernation and is automatically available when the agent is rehydrated. This is a platform-supplied property; the language relies on it but does not implement it.

From the type system's perspective, hibernation is transparent. A stored connection is a stored value; the platform's lifecycle machinery ensures the underlying resource is preserved across hibernation cycles. The compiler doesn't need to know about hibernation per se.

What the compiler does need to know: the platform makes specific guarantees about which held types survive which lifecycle events. `Connection[F]` survives hibernation; future held types may have different guarantees. The platform binding (§18 of design notes) is where these guarantees are concretely captured.

#### 2.9.7 Error paths and abnormal exit — Open

What happens to held values when a handler aborts (via `?` propagating Err, or via fault propagation, which may also trigger registered compensations from the `Sagas` capability)?

Provisional rules:

- *Held values owned by the handler at the moment of abnormal exit are implicitly consumed by the runtime.* The runtime closes them — sending a close frame on a Connection, releasing platform resources. This avoids resource leaks.
- *Held values stored in agent state are unaffected.* The transactional commit semantics mean the stored values either commit (the handler completed, abnormally or normally) or roll back (the agent's state is unchanged). If a handler aborts, any stored held values from earlier in the handler are rolled back.

The interaction with `Sagas.compensate` registered compensations: a compensation that wants to operate on a held value already in agent state can do so (the storage primitives' APIs work as expected). A compensation that tries to operate on a held value the handler had locally (before stuffing it into state) is operating on a value that's about to be consumed by the runtime; the compiler should reject this case statically.

Open: the precise rules for held values during fault propagation through cross-context calls; the diagnostics for the rejected case above; whether handlers can explicitly opt out of implicit consumption ("keep this connection alive across the abort, I'll deal with it"; probably no in v1).

#### 2.9.8 Worked example: the chat-room — Settled in shape

The chat-room from Example 2 of the design notes illustrates the discipline:

```
agent Room(id: RoomId) {
  store members:     Set[UserId]                              = {}
  store connections: Map[UserId, Connection[ServerFrame]]    = {}

  on join(u: UserId, conn: Connection[ServerFrame]) given Events {
    members.add(u)
    connections.put(u, conn)      -- conn: owned → consumed (transferred to storage)
    -- conn is no longer usable after this line
    Events.emit(UserJoined { ... })
  }

  on leave(u: UserId, _conn: Connection[ServerFrame]) given Events {
    connections.remove(u)         -- removes-and-closes the stored connection
    members.remove(u)
    -- _conn parameter is implicitly consumed at handler exit
  }

  on post(u: UserId, msg: Message) given Events {
    let conns <- connections.values.collect    -- borrows refs
    <- conns.parTraverse(c => c.send(MessageBroadcast(msg)))  -- borrows during iteration
    -- borrows end at parTraverse return; connections stay in the map
  }
}
```

Each held variable's lifecycle is visible at the operation level. `conn` in `join` is owned at parameter binding, consumed at `connections.put`. `_conn` in `leave` is owned at binding, implicitly consumed at handler exit. The `conns` in `post` is a list of borrowed references valid only during the parTraverse.

A reader can verify the discipline by inspection: every held binding goes from a creation site through a single chain to either a storage primitive's consume operation, an explicit consume, or implicit consumption at scope exit.

#### 2.9.9 Open questions

The major design is settled in shape (above). What remains:

- *Precise rules for held values during fault propagation across context boundaries.* The within-handler rules are clear; what happens to a held value that was being transferred to another context when a fault interrupts is open.
- *Diagnostic messages.* Held-resource errors need clear messages: "this branch leaves `conn` owned but the other branch consumes it" with suggested fixes.
- *Whether borrow-returning operations (e.g., `c.send(msg)`) are extensible to user-defined functions.* The current design says no — only platform-defined held operations are borrows; user functions take and return ownership. Open whether helper functions on held types could be classified as borrowing.
- *The notation for borrow vs owned types in user-facing diagnostics.* The compiler tracks the distinction internally; whether users ever see `&Held[T]` in error messages, or whether the discipline stays implicit, is open.
- *Whether `Held[T]` can be a generic parameter to user-defined types.* If a user wants to declare `type Subscriber = record { conn: Connection[ServerFrame], filter: Filter }`, does the record type carry the linearity discipline through? Probably yes (the record contains a Held field, so the record is itself Held-like), but the rules need spelling out.

### 2.10 Constrained refinement at other architectural points

Beyond refinement at type declarations (§2.5), refinement appears in three other places:

#### 2.10.1 Event subscription patterns — Open

```
from Events(PaymentConfirmed { region: Region.Domestic, .. })
```

Subscribers refine on event payload fields (and via envelope on schema version, see §7 of design notes). The refinement is structural — it matches values of the event type whose fields satisfy specified predicates.

Open: precise grammar and typing rules; how multiple subscribers' patterns interact at the runtime dispatcher.

#### 2.10.2 Agent invariants — Settled in shape

Agent invariants are predicates over the agent's storage state, declared in the agent body and checked at every handler commit. They are the language's mechanism for constraints that span multiple state fields, that depend on collection-aggregate properties, or that vary by variant — constraints refinement at type declarations cannot directly express.

**Declaration grammar.** Invariants are named declarations within an agent's body:

```
invariant-decl ::= 'invariant' Name ':' invariant-body

invariant-body ::= boolean-expr
```

The body is a boolean expression evaluated against the agent's tentative state at handler commit. The agent's `store` fields are in scope by their declared names, with the same implicit-deref sugar that applies in handler bodies (a `Cell[T]` referenced by name evaluates to its underlying `T`).

**Boolean operators.** The invariant body uses the standard boolean expression vocabulary:

- Comparison: `==`, `!=`, `<`, `<=`, `>`, `>=`
- Logical connectives: `&&` (AND), `||` (OR), `!` (NOT)
- `is` pattern testing (per §2.3.6) with type narrowing
- `implies` (right-associative, lower precedence than `||`)
- Method calls on values, refined-type predicates accessed via `.satisfies(P)` if needed
- Quantifiers over collections via `.all(...)` and `.any(...)` (which return `Effect[Bool]`)

The `implies` operator is logical implication: `A implies B` is `!A || B` semantically, with one important interaction — the narrowing introduced by `is` in `A` is in scope in `B`. This is the same rule as the general `is` operator: when `A`'s truth is what makes `B` evaluated, `A`'s bindings propagate.

Examples drawn from the worked agents:

```
invariant available_non_negative:
  available >= 0

invariant placed_has_user_and_cart:
  status == Placed implies (user.isSome() && cart.isSome())

invariant connections_subset_of_members:
  connections.keys.all(u => members.contains(u))

invariant confirmed_has_data:
  state is Confirmed(_, _, _, _, _) implies state.at > epoch

invariant held_implies_recent:
  state is Held(_, _, _, _, since) implies since > epoch
```

**Effectful body, total semantics.** Storage reads are effectful (per §2.7.1), so invariant bodies are evaluated in an effectful context — the body has type `Effect[Bool]` and the runtime awaits it. However, the body must be **total**: no operations that can panic (division-by-zero, out-of-bounds indexing, refined-type construction failures), no operations classified as writes (`put`, `update`, `:=`, `.append`), no cross-agent calls, no capabilities beyond pure read access. The compiler enforces totality and the no-writes/no-cross-agent restrictions statically:

- *No state writes.* The body may read storage but may not write. Any write operation in an invariant body is a compile error.
- *No cross-agent calls.* Invariants are agent-local. Calls to `Ref[A].method(...)` or capability operations are compile errors.
- *No held-resource consumption.* Operations that would consume a held value are compile errors. Read-only inspection (the kind producing borrowed references) is permitted but rarely useful.
- *No non-pure capabilities.* The `given` clauses available in handlers are not in scope. `Clock.now()`, `Random.next()`, etc., are not callable from invariant bodies — these would make invariants non-deterministic, which contradicts their role.

**Checking at handler commit.** At every handler commit:

1. The handler's tentative state changes are in the input gate (per the platform's atomic-handler semantics).
2. Each declared invariant is evaluated against the tentative state, in declaration order. Each evaluation awaits the body's `Effect[Bool]`.
3. If all return true, the gate commits and the handler completes.
4. If any returns false, the handler is treated as aborted: the `Sagas` provider's abort hook runs (firing any registered compensations from `Sagas.compensate` calls), state rolls back to pre-handler, the handler returns a fault to its caller. The first failing invariant is the one reported in diagnostics.
5. If invariant evaluation itself faults (a storage-level failure, an unanticipated runtime error), the same path applies: handler abort, state rollback, fault to caller. The fault attributes the cause to the invariant whose evaluation faulted.

The ordering matters only for diagnostics; semantically all invariants are conjoined (`inv1 && inv2 && ... && invN`).

**Short-circuit evaluation.** The boolean operators short-circuit:

- `A && B`: if `A` evaluates to false, `B` is not evaluated; result is false.
- `A || B`: if `A` evaluates to true, `B` is not evaluated; result is true.
- `A implies B`: if `A` evaluates to false, `B` is not evaluated; result is true (vacuously).

This is both a performance property and a safety property. An invariant like `state is Held(_) implies <expensive query>` only runs the query when state is in the `Held` variant; in other states, the predicate is vacuously true and the query is skipped. Developers can rely on this for invariants where the consequent is expensive or where the antecedent guards a precondition.

**Static proof and optimisation.** When a type-level refinement on a field makes an invariant statically provable, the compiler optimises out the runtime check and emits a warning:

```
bynk.invariants.statically_provable (warning):
  invariant `available_non_negative` is statically provable from the refinement on `available`
  (Cell[Int where NonNegative])
  the runtime check has been optimised out
  
  consider:
   - removing the invariant if the refinement makes it redundant
   - keeping it as documentation, accepting the redundancy
```

The warning surfaces the overlap between refinement and invariant, which usually indicates the constraint should live in one place rather than two. The developer chooses which.

**Fault diagnostics.** When an invariant fails at runtime, the diagnostic identifies the invariant by name, the handler in which the commit occurred, the expression body, and the values at violation:

```
bynk.runtime.invariant_violated:
  invariant: held_implies_recent (in agent Booking)
  handler:   hold (in context hotel.bookings)
  expression: state is Held(_, _, _, _, since) implies since > epoch
  values at violation:
    state = Held(guest_xyz, room_402, ..., 1970-01-01T00:00:00Z)
    since = 1970-01-01T00:00:00Z
    epoch = 1970-01-01T00:00:00Z
  handler tentative state has been rolled back; fault returned to caller
```

When an invariant evaluation faults rather than returning false:

```
bynk.runtime.invariant_evaluation_faulted:
  invariant: connections_subset_of_members (in agent Room)
  handler:   broadcast (in context chat)
  cause:     storage read failure (Map.keys on connections)
  handler tentative state has been rolled back; fault returned to caller
```

The two are distinct because they're different operator concerns: violations indicate the application logic produced state that shouldn't have been allowed; evaluation faults indicate an infrastructure problem.

**Compile-time diagnostics** when an invariant body is malformed:

```
bynk.types.invariant_writes_storage:
  invariant body cannot perform storage writes
  found: members.add(u) on line 47
  invariants are read-only over state; consider whether this belongs in the handler instead
```

**Relationship to refinement.** Invariants are complementary to type-declaration refinement (§2.5). Use refinement when the constraint is over an individual value's shape or range and can be carried in the type identity; use invariants when the constraint relates multiple fields, involves collection aggregates, or varies by variant. The compiler's static-proof optimisation flags places where the two overlap.

**Composition with the Sagas capability.** Invariant violations and faults during commit run the handler's registered `Sagas.compensate` actions before the state rollback (per §13 of design notes), via the Sagas provider's abort hook. Compensation actions for remote effects therefore still execute. The combination provides: state rolls back locally (atomic commit didn't happen); remote effects are compensated via the registered Sagas actions; the caller receives a fault. The handler's behaviour is consistent whether the abort came from a domain Err, a fault, or an invariant violation.

#### 2.10.3 Capability operation refinements — See §2.6.5

Framework-internal; specified at the capability declaration site.

---

## 3. Checking order

The type checker proceeds in phases:

1. *Parse and build the AST.*
2. *Resolve context structure.* Build the context dependency graph; verify acyclic; determine the order of context checking.
3. *Resolve type names within each context.* Bind type declarations to their declared meanings; check exports refer to declared names.
4. *Resolve cross-context type references.* Verify imported types are exported from their declaring contexts.
5. *Check refinement predicates.* Verify each refinement uses known predicates with correct argument types.
6. *Type-check declarations in dependency order.* Within each context, declarations may reference each other; the checker uses a fixpoint or topological order.
7. *Type-check function bodies and handler bodies.* Bidirectional inference; produce a fully annotated AST.
8. *Check capability resolution.* Verify each `given C` resolves to exactly one `provides C` in scope.
9. *Check refinement propagation.* For operations on refined types, apply the propagation rules (§2.5.4); insert validation calls where refinement is reintroduced.
10. *Check invariant satisfaction.* For each handler, verify that the agent's invariants hold at commit (statically where provable; otherwise insert runtime checks).
11. *Check linearity for held resources.* Verify the linearity discipline (§2.9.2) on Held values.
12. *Emit results.* Produce a typed AST suitable for codegen; emit diagnostics for any errors.

Open: the precise interaction between phases (especially refinement propagation and effect inference); incremental re-checking when source changes.

---

## 4. Diagnostics

The type checker produces diagnostics in a canonical structured form for downstream tooling (LSP, dev server, build logs). Each diagnostic carries:

- *Severity*: error, warning, info, hint.
- *Source location*: file, line range, column range.
- *Code*: a stable identifier for the kind of diagnostic (e.g., `bynk.types.refinement.unprovable`).
- *Message*: human-readable summary.
- *Detail*: extended explanation when relevant (e.g., for refinement-propagation failures, which predicate couldn't be derived).
- *Suggestions*: optional code-action proposals (e.g., "wrap in `T.of(...)?`").

Open: the precise diagnostic taxonomy; the format for renderer consumption; localisation.

---

## 5. Open questions (consolidated)

The questions identified throughout this document, grouped:

*Core type system*
- Anonymous structural records: admitted in narrow positions only, or excluded entirely (§1.7).

*Bounded contexts*
- Import/aliasing syntax (§2.1.2).
- Re-export rules (§2.1.2).
- Cyclic dependency policy (§2.1.2).
- Visibility of type parameters of exported types (§2.1.3).

*Opaque types*
- Representation declaration form (§2.2.1).
- Companion-function visibility rule (§2.2.3).

*Closed sums*
- Variant-level type parameters (deliberately deferred to post-v1).
- View patterns (likely deferred; not currently needed).

*Records*
- Record update syntax (§2.4.2).

*Refinement* (the largest area)
- Predicate vocabulary for v1 (§2.5.2).
- Propagation rules for `Matches` predicate (§2.5.4) — most cases non-preserving; the exact rules need to be written down.
- Layered refinement (§2.5.6).
- Boundary error formats (§2.5.7).
- Schema generation mappings (§2.5.9).

*Capabilities*
- Resolution disambiguation (§2.6.3).
- Provider declaration syntax (§2.6.3).
- `given` inference on helpers (§2.6.4).
- Capability operation refinement vocabulary (§2.6.5).

*Storage types*
- Precise operation signatures continued refinement (§2.7.3) — except the `Cell` operations, normative as of v0.98 (ADR 0125).
- Initial-value edge cases for fields needing Effect-typed construction (§2.7.4).

*Built-in value types*
- The full method surface for `Option`, `Result`, `List` (§2.7.6).

*Effects*
- Effect inference rules (§2.8.4).
- The parsing-level treatment of `<-` and `?` combinations (§2.8.3) — currently disambiguated by type-system; whether to commit to a single grammar reading.

*Held resources*
- The model is now settled in shape (§2.9.1 through §2.9.8). What remains: fault-propagation semantics across context boundaries (§2.9.7); diagnostic messages for linearity errors; whether user-defined functions can be classified as borrowing operations; record types containing held fields; the user-facing notation for borrows in diagnostics. See §2.9.9 for the consolidated list.

*Other refinement points*
- Event subscription pattern grammar (§2.10.1).
- Invariant body details (§2.10.2): the static-proof algorithm — which refinements imply which invariant predicates, how the optimisation detects redundancy. The shape is settled; the precise detection rules are remaining work.

*`Sagas` capability* (cross-cutting; not yet a section of this spec)
- Lexical scoping of `Sagas.compensate(...)` calls inside helper functions called by handlers — do helper-internal registrations propagate to the handler's saga stack, or stay local to the helper? The provisional rule (helpers handle their own partial-failure semantics; only the handler's lexical Sagas.compensate calls register on the handler's stack) is consistent with the existing examples but isn't formally specified.
- Serialisability of compensation closures (§2.5.8): the rule is specified, but the compiler-side check at `Sagas.compensate(...)` call sites for handlers binding a durable Sagas provider is a new pass that needs to be planned.

*Diagnostics*
- Diagnostic taxonomy and rendering (§4).

---

## 6. Cross-references to design notes

For rationale, alternatives considered, and architectural commitments, see:

- §15 of design notes: Type System (the architectural overview that this spec implements).
- §8: Bounded Contexts.
- §10: Storage Types.
- §13: Failure Model (for `Result` / `Effect` / `Fault` and their interactions).
- §14: Validation (for invariants).
- §7: Services and Protocol Composition (for capabilities, event subscriptions, boundary validation).
- §18: Runtime and Platform Relationship (for first-party capabilities, framework-internal refinements).
- §20: Worked Examples (for concrete demonstrations of types in use).

---

*End of working draft.*
