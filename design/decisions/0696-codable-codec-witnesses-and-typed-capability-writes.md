# 0696 — `Codable` gives generic code the boundary codec through a hidden witness, and capability operations take `Codable` parameters encoded at the call site

- **Status:** Accepted (v0.478)

**Context.** Only the compiler knows how a type crosses the boundary: it generates
and validates the codecs per type, refinements included. Bynk generics are erased, so
a user could not write `encode[T]`, and a type variable was
`bynk.types.json_uncodable`. Two things followed. Generic code could not wrap a
decode (standard-library D4: "every call site writes its own `Json.decode`"), and the
stdlib could not offer a typed `Queue.send[T]`, because ADR 0281 shipped generic
capability operations with no codec story and Part 14 clause (iv) refused a
compiler-synthesised argument. This is slice 3 (#2715) of the constrained-generics
track (`design/tracks/constrained-generics.md`, Q4, Q7 and §8), after slice 0's
bounds and `Eq`, slice 1's `Ord` witnesses and slice 2's `Key`. The maintainer
approved #2715 as written: decisions A to J as recommended, with J1 and H1.

**Decision.**
1. **What is `Codable` (A).** Exactly the domain `Json.decode` accepts, moved into one
   predicate (`checker/codable.rs`, `json_codable`) that `Json.encode`, `Json.decode`
   and the bound share, so the `Json` rule and the bound rule cannot drift. Base
   types, refined and opaque types, `Option`, `List`, `Set`, `Map` and `Result` of
   `Codable`, records, sums, enums and generic instantiations whose arguments are
   `Codable`, and a type parameter declared `T: Codable`. Not `Codable`: top-level `()`
   (it encodes but does not decode), functions, `Effect`, `Query`, `Stream` and
   anything carrying one, `Connection`, the HTTP and queue result types, `Asset`,
   request headers, token claims, statements, actors, a recursive generic and an
   unbounded type parameter. An unsatisfied instantiation is
   `bynk.types.not_codable`; `Json.*` keep their own codes and accept exactly what
   they accepted before. The declaration-position rule (`reject_fn_types`) stays
   separate. `Json.encode` of a crypto key (`SigningKey`, `VerifyKey`) is accepted
   today, because the key check lives only in `reject_fn_types`; this slice does not
   change that, so such a key satisfies `Codable`. Tightening both is a follow-up.
2. **The witness (B).** One hidden trailing argument per `Codable` parameter,
   `__codec_T: { encode: (v: T) => __JsonValue; decode: (j: __JsonValue, path: string)
   => Result<T, __BoundaryError> }`, built at the call site from the concrete type's
   generated `__serialise_X`/`__deserialise_X` and forwarded by a rigid parameter.
   Witness order is fixed so units agree: the receiver type's parameters then the
   function's own, each in declaration order, and within one parameter the comparator
   before the codec (`CONSTRAINTS` order). `[A: Codable, B: Ord]` takes
   `__codec_A, __cmp_B`; `[T: Ord + Codable]` takes `__cmp_T, __codec_T`. One function,
   `witness_list`, defines it for the checker and the emitter.
3. **Containers over a type parameter (C).** `List[T]`, `Option[T]`, `Set[T]`,
   `Map[K, T]` and `Result[T, E]` get their codec from runtime combinators
   (`codec.ts`: `__bynkCodecList`, …) over the witnesses of their parts. Each produces
   the JSON and the errors, at the same paths, as the monomorphised helper for the same
   concrete type, including `Option`'s lenient decode and the `Set` duplicate check;
   a `Set` or `Map` is rebuilt as a plain or keyed collection chosen by its first
   element, as `__bynkSetOf` does. A behaviour fixture compares the two paths per
   container, including a composite key. A user generic type over a type parameter
   (`Box[T]` in `fn f[T: Codable]`) would need a codec factory per generic declaration;
   it is `bynk.generics.codec_generic_at_rigid`. Concrete instantiations are unaffected.
4. **Capability operations (D).** An operation's type parameter may carry `Codable`
   (and `Key`; `Eq` and `Ord` remain `bynk.generics.bound_on_capability_op`). The call
   site encodes each argument whose declared parameter type mentions a `Codable`
   parameter, whole, with the codec of its instantiated type (`__serialise_Order(order)`,
   or `__serialise_List_Order(xs)` for `List[T]`), and the interface emits that
   parameter as `unknown`. Other parameters pass through unchanged. A type parameter
   that only occurs in encoded parameters is dropped from the emitted interface, so no
   unused generic is left. The provider receives plain JSON and never a witness, so
   the emit ABI (ADR 0310) does not grow and no `__codec_` name appears in any
   capability interface; a test asserts it. As an addition, an operation with `Codable`
   parameters infers its type arguments from its arguments when they determine all of
   them (`Queue.sendValue(order)`); other operations still require them explicitly.
5. **Return types (E).** A `Codable` parameter in an operation's return type is
   `bynk.generics.codable_op_return`. Decoding a provider's reply raises what a failed
   decode of stored data means, which this slice does not answer. Typed reads wait.
6. **Missing bounds (F).** `Json.encode`/`Json.decode[T]` on an unbounded type
   parameter, or a rigid `T` passed to a `Codable` parameter, is
   `bynk.generics.missing_codable_bound`, with a machine-applicable fix that `bynk check
   --fix` and the editor apply. Unlike `Ord` and `Key`, `Codable` does not imply `Eq`,
   so the fix adds `Codable` beside an existing `Eq` rather than replacing it.
7. **ADR 0281 (G).** E is reaffirmed: a generic operation still needs an external
   provider, because a Bynk-bodied one would need `T` threaded through
   `check_handler_body`. D ("no codec/persistence story") is superseded for `Codable`
   parameters.
8. **Stdlib (H1).** `bynk.cloudflare` gains `Queue.sendValue[T: Codable](message: T)`.
   Its binding passes the JSON value through `JSON.stringify` and `JSON.parse` before
   `producer().send(…, { contentType: "json" })`, which is the text `Json.encode` writes
   and `send` parses, so the queue receives exactly the message `send(Json.encode(v))`
   sends (an absent field stays absent, `-0` is `0`). Tests in the binding suite and the
   workerd suite compare the two.
9. **Opaque types outside their owner (I).** A witness built in a consuming unit
   re-checks an opaque type's predicate no more than a monomorphic boundary there does
   (`StructuralOnly`). This slice does not weaken it; closing the gap for both paths
   together is a follow-up.
10. **Part 14 clause (iv) and standard-library D4 (J1).** Clause (iv) is amended: *an
    encoding derived from a `Codable` bound declared in the operation's own signature,
    using the boundary codec, is not a compiler-synthesised argument.* The argument:
    clause (iv) targets per-capability compiler knowledge (`Idempotency`'s key is
    synthesised from facts only the compiler has about that one capability). A `Codable`
    operation is different: any adapter can declare it, it is visible in the signature,
    it lowers by one rule for every capability, the provider receives a plain JSON value,
    and nothing joins the handler transaction. D4 records that a `Codable` operation is
    not Tier 2. D4's read-side condition (a second consumer for typed reads) stays in
    force, and typed reads stay deferred by decision 5. `Idempotency`'s key and
    `Locale`'s payload remain outside the amendment.

**Consequences.** Generic code can encode and decode, and `Queue.sendValue` is the
first typed capability write; typed reads (`Kv.get[T]`, D1 `query[T]`) and
`Analytics.write[T]` are follow-ups. A generic function with a `Codable` bound has an
extra trailing parameter, like an `Ord` one; bindings never call bounded Bynk generics
directly. A base type's witness decoder is the one used for a record field or container
element, so a top-level `roundTrip[Int]` reports a non-number as `expected number` where
the monomorphic top-level decoder says `expected safe integer`; only that message differs.
Unbounded code emits byte-identically. Every vendored `bynk.cloudflare` output moves,
because `Queue` gains an operation. A capability call inside a generic function is
not reachable today (only handlers hold `given`), so the lowering through a forwarded
witness at a capability call is in place but unexercised.
