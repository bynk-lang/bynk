# Constrained generics — declared bounds on type parameters

- **Status:** Draft (settling). Direction not yet merged; no slice authorised.
  Spine [#2614](https://github.com/bynk-lang/bynk/issues/2614). The questions in
  §5 carry *draft* recommendations for the settling review; none is settled
  until this PR is marked ready and merged.
- **Realises:** the extension point [ADR 0028](../decisions/0028-open-narrow-generics.md)
  left open ("the `TypeParam` representation is a struct, so a bound is a later
  field addition, not a rework"), and the alternative
  [ADR 0430](../decisions/0430-equality-bounds-at-instantiation.md) §1 deferred
  rather than rejected ("Explicit `T: Eq` bounds were the alternative, and a
  larger language change").
- **Posture:** Feature track per [ADR 0076](../decisions/0076-feature-track-posture.md).
  It qualifies on two axes (§2): it spans several increments, and its surface is
  not settled. It sits in the Foundations layer that the
  [1.0 definition](../bynk-1.0-definition.md) freezes, which is why the
  load-bearing calls are settled here rather than cut straight into proposals.
- **Governing records:** ADR 0028 (open-narrow generics, no bounds), 0029
  (argument-directed inference), 0183 / 0197 (generic records and their
  monomorphised codecs), 0234 (generic instance methods), 0237 (generic sums),
  0281 (generic capability operations), 0310 (only the four-item emit ABI is
  published), 0421 (structural `==` on equality-supporting types), 0430
  (inferred equality bounds checked at instantiation).

## 1. The theme

A Bynk generic can move a value of type `T` around but can do almost nothing
*with* it. Type parameters exist on functions, records, sums, instance methods
and capability operations, but a bound is a parse error
(`bynk.generics.no_bounds`, in `parse_optional_type_params`, `bynk-syntax/src/parser/types.rs`). The one
exception is equality: ADR 0430 *infers* an "is compared" bound on a type
parameter and checks it at every call site.

Every other thing a generic might need to know about `T` is either impossible
or special-cased in the compiler for a fixed list of types:

| Need | What happens today |
|---|---|
| **Order** `T` | `sortBy`, `min`, `max` take a key function whose result must be `Int`, `Float`, `String`, `Duration` or `Instant` (`bynk.types.key_not_orderable`). There is no `sort()` on a `List[T]` and no `<` on a record, enum or `T`. |
| **Key** a `Map` or `Set` by `T` | Keys and members must be `String`, `Int`, or a refined/opaque type over them (`bynk.types.unkeyable_map_key`, `bynk.types.unkeyable_set_element`, `check_keyable` in `bynk-check/src/resolver.rs`). The diagnostic's own note says "value-equality keys need bounded generics". |
| **Encode or decode** `T` | `Json.decode[T]` is a compiler-backed static. A generic function or capability operation cannot require "`T` is boundary-legal", so the stdlib cannot offer a typed `Queue.send[T]` or `Analytics.write[T]` (`design/tracks/standard-library.md` §6.3), and ADR 0281 D rules out any codec for a generic capability op's `T`. |
| **Display** `T` | Interpolation accepts only scalars (`bynk.types.interpolation_non_scalar`). A user type cannot say how it renders. |
| **Static methods on generic types** | Deferred (`bynk.generics.method_on_generic_type`, in `bynk-check/src/resolver.rs`), because nothing supplies the type's parameters without a receiver. |

### 1.1 What this track settles

1. **The model.** Whether constraints are a closed, compiler-known set or an
   open, user-declarable interface mechanism (§5 Q1), and how a type comes to
   satisfy one (Q2).
2. **The syntax and its relationship to inference.** How a bound is written, and
   what happens to ADR 0430's inferred equality bound (Q3).
3. **The runtime shape under erasure.** Bynk generics are not monomorphised in
   the emitted TypeScript, so any constraint that needs per-type behaviour at run
   time needs a strategy (Q4). This is the hardest call to reverse.
4. **Each constraint's meaning**: ordering (Q5), keying (Q6), and codec (Q7).
5. **What must land before 1.0** (Q8).

**End state when this track retires:**

- a bound can be written on any type parameter and is checked at instantiation;
- `List[T].sort()`, `min()`/`max()` and the comparison operators work for any
  orderable type, user types included;
- records, enums and `Bytes` can key a `Map` and be `Set` members;
- the stdlib offers codec-carrying generic operations (`Queue.send[T]`);
- static methods on generic types are supported;
- unbounded generics emit exactly as they do today.

## 2. Why a track (the ADR 0076 trigger)

- **Multi-increment.** The bound syntax and checker come first, then each
  constraint (ordering, keying, codec, display) with its own runtime and
  stdlib uptake, then static methods on generic types. Each is a separate
  fixtures-and-emission story.
- **Surface not settled.** Closed versus open constraints, structural versus
  declared satisfaction, and the runtime witness strategy are all genuinely
  open (§5). Prior art splits sharply: Go and Swift-with-synthesis on one side,
  Rust and Haskell on the other, Gleam and Elm deliberately without either.
- **Not a security boundary** in the usual sense. The codec constraint (Q7)
  touches the boundary decoder, and §8 records what it must not weaken.

## 3. What's already true

- **Parameters.** `struct TypeParam { name, span }` (`bynk-syntax/src/ast.rs`).
  The parser reads `[A, B]` on `fn` and `type` declarations and rejects `[A: …]`
  at the colon with `bynk.generics.no_bounds`.
- **Rigid variables.** Inside its body a type parameter is equal only to itself
  (§5.2). The checker type-checks generic records and sums by substitution
  (ADR 0183, 0237) and erases them to TS generics.
- **Codecs are already monomorphised.** A generic record or sum instantiation
  crossing a boundary gets a per-instantiation codec (`serialise_Paginated_User`,
  ADR 0197, 0237). So the emitter already knows how to specialise per concrete
  type at a *boundary site*, but not inside a generic function body.
- **Equality.** `==` is structural (ADR 0421). A generic body's `==` on a `T`
  emits `__bynkEq` (`bynk-emit/runtime/src/equality.ts`), a runtime walk over
  self-describing values. The file records why a type-directed `equals_T` was
  rejected: "Bynk generic functions are not monomorphised … so `==` on a type
  parameter has no concrete type to dispatch on. The in-memory representation
  describes itself instead." ADR 0430 then infers which type parameters a
  generic compares, transitively and across units, and checks each call's type
  arguments against them.
- **Ordering.** Orderability is a fixed list (`bynk.types.key_not_orderable`):
  `Int`, `Float`, `String`, `Duration`, `Instant`. The comparison operators are
  defined on same-typed `Int`, `Float`, `String` operands (§5.2). `Bytes` has a
  `compare` *method* but is explicitly "equatable but not orderable"
  (`design/bynk-type-system.md`).
- **Keys.** `Map` and `Set` keys follow the value-keyable rule above. Agent keys
  are wider: a record can be an agent key, and the runtime's
  `serialiseAgentKey` (§7.4.4) already serialises keys canonically
  ("semantically-equal keys (records compared by sorted fields) MUST serialise
  identically"). That is a working precedent for canonical key encoding.

## 4. Constraints on any answer

These come from existing principles and decisions, not from this track.

- **One canonical way.** A constraint mechanism must not leave two equally good
  spellings of the same requirement. That matters for Q3, where inferred `==`
  already exists.
- **Cost visible at the call site.** A requirement a caller must meet should be
  readable from the callee's declaration, not discovered from a diagnostic.
- **Unbounded generics stay free.** Today a generic function is an erased TS
  generic with no extra arguments. Code that does not use bounds must emit
  byte-identically, both for golden stability and so that bounds are pay-as-you-go.
- **The published ABI does not grow by accident.** ADR 0310 publishes four
  items. A witness or comparator shape is internal unless a later ADR publishes it.
- **Foundations freeze.** Generic signatures in L0 kernels and L1 commons are
  Foundations-level under the 1.0 stability tiers. Anything that changes how an
  existing generic signature is written must land before 1.0 or be additive.

## 5. Open design questions

Each question has options, prior art, and a **draft recommendation** for the
settling review to test.

### Q1 — Closed or open?

- **(a) Closed.** A fixed, compiler-known set: `Eq`, `Ord`, `Key`, `Codable`,
  perhaps `Show`. Users cannot declare new constraints. A type's satisfaction is
  derived (Q2). Prior art: Go's `comparable`/`cmp.Ordered`; Swift's synthesised
  `Equatable`/`Hashable`/`Codable`.
- **(b) Open.** User-declared interfaces or traits with user implementations,
  usable as bounds. Prior art: Rust traits, Haskell typeclasses, Swift protocols.
- **(c) Neither.** Keep passing functions explicitly (`sortBy(key)`), and widen
  the fixed lists ad hoc. Prior art: Gleam, Elm (whose `comparable` is a closed
  set in all but name).

**Draft recommendation: (a), closed.** Every motivating case in §1 is one of
four or five well-known constraints whose meaning can be derived from a type's
structure. An open mechanism brings implementation coherence, orphan rules,
dictionary-passing everywhere and a large new Foundations surface to freeze,
for expressiveness no current program needs. The closed set can grow by ADR,
and Q4's witness design should not preclude (b) later.

### Q2 — How does a type satisfy a constraint?

- **(a) Structurally, always.** As `==` already works: a record is `Ord` when all
  its fields are, a sum when all its payloads are.
- **(b) By declaration.** `type Priority = enum { Low, Medium, High } derives Ord`.
- **(c) Structural for `Eq`, `Key` and `Codable`; declared for `Ord`.**

Structural ordering makes **declaration order semantic**: a record compares by
its first field, then its second, and an enum by variant order. Today field
order and variant order explicitly carry no meaning (the wire-format reference
lists "field order; variant order" as not changing the contract hash).
Reordering fields would silently change sort order.

**Draft recommendation: (c).** `Eq`, `Key` and `Codable` have one obvious
structural meaning and no ordering hazard, so they stay automatic, consistent
with ADR 0421. `Ord` is opted into with `derives Ord`, which makes declaration
order deliberate and visible exactly where it becomes meaningful. A refined
type over an orderable base is `Ord` without declaration: its representation is
its base, visible everywhere. An opaque type is not, because ordering it by its
hidden representation would expose that representation's order outside the
defining `commons`. Its defining `commons` opts in with `derives Ord`. The review
should confirm this against how `sortBy` treats opaque keys today, and keep the
two consistent.

### Q3 — Syntax, and what happens to inferred equality

The bound syntax is `fn sort[T: Ord](xs: List[T]) -> List[T]` and `T: Ord + Key`
for several. The real question is ADR 0430:

- **(a) Keep `==` inferred; other constraints declared.** Two models for one
  concept.
- **(b) Make `Eq` declared too, and retire the inference.** That breaks every
  existing comparing generic until it adds `: Eq`. It is codemod-able, since the
  inference already computes exactly the bound to write.
- **(c) Infer all constraints from use.** `<` on a `T` infers `Ord`, `Map[T, _]`
  infers `Key`, and so on. No new syntax, but nothing in the signature tells a
  reader what is required.

**Draft recommendation: (b), with a codemod, before 1.0.** "Cost visible at the
call site" and "one canonical way" both point to declared bounds. The inference
machinery is kept as the codemod's engine and as the source of a quick fix
("this generic compares `T`; add `T: Eq`"). This is a breaking change to
Foundations-level source, so it is the main fork for maintainer sign-off and the
reason Q8 exists.

### Q4 — The runtime shape under erasure

Equality avoided this question because values describe themselves. Ordering
cannot: a nullary variant is a string tag with no position, and a record's JS
key order is not a reliable declaration order. A generic `sort[T: Ord]` needs
the comparator for the *concrete* `T` at run time.

- **(a) Self-describing values.** Emit variant index and field order into the
  runtime representation. That changes the in-memory shape of every sum and
  record, and the stored-state encoding Gate 3 freezes (ADR 0538). Rejected.
- **(b) Witness passing for bounded parameters only.** A parameter bound by a
  constraint that needs run-time behaviour (`Ord`, `Codable`, `Show`) gets one
  hidden argument per parameter: a compiler-generated witness (comparator,
  codec pair, renderer) for the concrete type at each call site, threaded
  through nested generic calls. Unbounded parameters, and `Eq`/`Key` (served by
  `__bynkEq` and canonical key encoding), add nothing.
- **(c) Monomorphise bounded generics.** Emit one copy per instantiation, like
  the boundary codecs. That gives bigger output and makes cross-unit generics
  and recursion harder.

**Draft recommendation: (b).** It confines the cost to code that asks for it,
keeps unbounded output byte-identical (§4), and the per-type witnesses reuse what
the emitter already generates (the monomorphised codecs of ADR 0197/0237 are
exactly a `Codable` witness). Witnesses are internal and not added to the ADR
0310 ABI. The settling review should confirm that bindings never call bounded
Bynk generics directly; if they can, the witness argument becomes ABI.

### Q5 — What `Ord` means

- The comparison is total and, for every type except `Float`, consistent with
  `==`. `Float` cannot have both: IEEE `==` makes `NaN != NaN`, while a total
  order must have `compare(NaN, NaN) == 0`. The options are:
  - exclude `Float` from `Ord` (Rust's `PartialOrd` split), which breaks
    today's `Float` `sortBy` keys;
  - give `Float` a total order that agrees with `==` everywhere **except at
    `NaN`**: `-0` and `0` compare equal, and `NaN` compares equal to itself and
    greater than every other value;
  - change `Float`'s `==` to match that total order, which contradicts the IEEE
    semantics §5.2 specifies.

  **Draft:** the second option. Keep `Float` orderable, and state the `NaN`
  exception normatively. `sort` puts `NaN`s last, `max` returns `NaN` when one
  is present, `min` returns `NaN` only when every element is `NaN`, and the
  comparison operators keep their IEEE meaning (`NaN < x` is `false`). `==` is
  unchanged. The review should weigh this against excluding `Float`.
- Records order lexicographically by declared field order; sums by variant
  declaration order, then payload; `List` lexicographically; `Option` puts
  `None` first; `Bytes` unsigned lexicographically (matching its existing
  `compare`). `Map`, `Set`, functions and held resources are not `Ord`.
- The comparison operators `< <= > >=` extend to any `Ord` type. `sortBy`
  remains for key extraction; `sort()`, `min()` and `max()` are added on
  `List[T: Ord]` in `bynk.list`.

### Q6 — What `Key` means

`Key` admits a type as a `Map` key or `Set` member. It requires `Eq` plus a
canonical encoding, so that semantically equal values index identically.

- **Draft:** `Key` holds for every equality-supporting, boundary-legal type
  except those containing `Float` (where `NaN != NaN` breaks lookup). The runtime
  encodes non-primitive keys with a canonical encoding **derived from**
  `serialiseAgentKey` (`bynk-emit/runtime/src/agent.ts`), not that function as
  it stands. It has two properties a general key encoding cannot keep:
  - A top-level `String` is returned raw while other kinds are JSON, so `"1"`
    and `1` encode alike. That is safe while one map has one key type, but not
    if an encoding ever spans types, for example sum payloads of different kinds
    behind one tag.
  - `Bytes` falls into the generic-object branch and encodes as index→byte
    pairs sorted as strings (`"10"` before `"2"`). That is deterministic but
    large and slow; a dedicated `Bytes` form (hex or base64) is better.
- Open: whether widening keys changes the **stored-state encoding** of
  `Map`-typed `store` fields. Primitive-keyed maps must keep their encoding
  exactly, and a non-primitive-keyed map is new, so this should be additive. The
  settling review must confirm it against ADR 0538.
- Open: iteration order. Insertion order is unchanged, which this track
  recommends keeping.

### Q7 — `Codable`, and generic capability operations

`Codable` holds for every boundary-legal type, the domain `Json.decode` already
checks (`bynk.types.json_uncodable`). Its witness is the serialise/deserialise
pair the emitter already monomorphises per type.

- It lifts ADR 0281 D for bounded parameters: `capability Queue { fn send[T:
  Codable](msg: T) -> Effect[()] }` can encode `msg` without the caller calling
  `Json.encode`.
- It keeps ADR 0281 E (a generic op needs an external provider) unless the
  settling review finds the witness makes Bynk-bodied providers tractable.
- **Draft:** Slice 3, after ordering and keying, because its main consumer is
  the stdlib track (#1936) and it can be cut as a stdlib slice once the
  mechanism exists.

### Q8 — What must land before 1.0?

If Q3 resolves to (b), declared `Eq` changes existing Foundations-level
source, so the bound syntax and the `Eq` cutover (slice 0) must ship before the
1.0 freeze. Every other slice adds a new capability without breaking existing
code, so it can land in 1.x. If Q3 resolves to (a) or (c), the whole track is
additive and nothing gates 1.0.

## 6. Candidate slice decomposition

Provisional until this doc settles. Each becomes an increment proposal opened as
a sub-issue of #2614.

- **Slice 0 — Bounds and `Eq`.** Parse and check `T: C`; the constraint table;
  checking at instantiation; `Eq` as the first constraint, with the ADR 0430
  codemod and quick fix if Q3 resolves to (b). No runtime change.
- **Slice 1 — `Ord`.** `derives Ord`; the normative total order (Q5); witness
  passing (Q4); comparison operators on `Ord` types; `sort`/`min`/`max` in
  `bynk.list`.
- **Slice 2 — `Key`.** Widened `Map` keys and `Set` members; canonical key
  encoding; the stored-state check from Q6.
- **Slice 3 — `Codable`.** Codec witnesses; lifting ADR 0281 D; the first typed
  generic stdlib operation, cut jointly with #1936.
- **Slice 4 — Static methods on generic types.** Resolve the type's parameters
  from explicit type arguments or the expected type (`Box.empty[Int]()`).
- **Slice 5 — `Show`**, only if the settling review keeps it: a user-defined
  string form usable in interpolation.

## 7. Front-loaded ADR candidates

To land with, or just before, slice 0:

- **Closed constraint set and satisfaction model** (Q1, Q2). This fixes the
  shape of the Foundations surface.
- **Declared versus inferred `Eq`** (Q3). It decides whether slice 0 is breaking.
- **Witness passing for bounded parameters** (Q4). Hard to reverse once emitted
  code and the stdlib depend on it. It can land with slice 1, but must be
  decided before slice 0 so the constraint table is shaped for it.

## 8. Threat model

None primarily, because constraints are a static, erased mechanism. Two
obligations follow from Q6 and Q7:

- **`Codable` must not weaken the boundary decoder.** A witness is the same
  generated codec a monomorphic boundary uses, so untrusted input is still
  validated against the concrete `T`, refinements included. A witness must never
  decode against the type parameter itself.
- **Canonical key encoding must be collision-free.** Two values unequal under
  `==` must never encode to the same key, or one `Map` entry silently shadows
  another. The encoding needs a fixture for each kind: record field-order
  independence, nested sums, `Bytes`, and a `String` against a
  numeric-looking string (`"1"` against `1` inside a sum payload).

## 9. Slice status

- [x] Slice 0 — bounds and `Eq` (#2639)
- [ ] Slice 1 — `Ord`
- [ ] Slice 2 — `Key`
- [ ] Slice 3 — `Codable`
- [ ] Slice 4 — static methods on generic types
- [ ] Slice 5 — `Show` (conditional)

## 10. Non-goals

- **User-declared interfaces or traits** (Q1 (b)). Not ruled out forever, but
  not this track. Q4's witness design should leave room for them.
- **Higher-kinded types** (`F[_]`), associated types, and constraint-based
  overloading. ADR 0174's decision against return-type overloading stands.
- **Numeric constraints** (`T: Num` for generic arithmetic). `Int` and `Float`
  stay distinct with no coercion (ADR 0041), and generic arithmetic would
  reopen that.

## 11. Done when

- Every slice in §9 has shipped or been explicitly dropped with a recorded
  reason.
- The spec states the constraint set, the satisfaction rules, the total order and
  the key encoding normatively, in spec chapters 5 to 7.
- Unbounded generic code emits byte-identically to before the track, proved by
  the existing goldens.
- The retirement PR removes this doc, appends its closing summary to
  `archive/retired-tracks.md`, and closes #2614.
