---
level: minor
probes: emit_diagnostics, diagnostic_coverage, ast_importers
changelog: "**Static methods on generic types, and the expected type as the last source of type arguments** (constrained-generics slice 4): `fn Box.of(value: T) -> Box[T]`, `fn Box.empty() -> Box[Option[T]]`, `fn Sorted.fromList(xs: List[T]) -> Sorted[T]`. A static on a generic type is generic over the type's parameters, then its own. They come from the arguments (`Box.of(5)`), from the expected type (`let b: Box[Option[String]] = Box.empty()`), or from explicit type arguments (`Box.empty[String]()`: the type's parameters, then the method's own, all or none). A bound the type declares adds the usual hidden witness at the call site (`Sorted.fromList(names)` passes the `String` comparator). The expected type now also grounds a parameter the arguments leave open for every generic call except a capability operation, so `let o: Option[Int] = none()` works for `fn none[T]() -> Option[T]`; `let o = none()` is still `bynk.generics.uninferable_type_arg`. `bynk.generics.method_on_generic_type` is narrowed to a static that never mentions one of its type's parameters. `Opt.None[Int]` and explicit type arguments on an instance method stay rejected. Code that uses no static on a generic type is emitted unchanged"
---

## ADR: static-methods-on-generic-types
title: A static method on a generic type is generic over the type's parameters, and the expected type is the last source of a generic call's type arguments
summary: statics on generic types, explicit type arguments `Box.empty[Int]()`, call-site witnesses for statics, the narrowed `method_on_generic_type`, and the expected-type rule for every generic call except capability operations (constrained-generics slice 4)

**Context.** A generic type with no way to build it except a literal cannot hide its
representation: `Sorted[T]` is sorted only if `Sorted.fromList` is the way in. ADR 0234
C left a static method on a generic type deferred, because it has no receiver to supply
the type's parameters, and ADR 0183 D rejected methods on generic types altogether. The
workaround was a free function beside the type under an invented name, so a generic
type's constructors and a plain type's (`Counter.zero()`) had two spellings. Slices 0
to 3 (#2645, #2672, #2710, #2724) made bounds on a type's parameters meaningful, and a
constructor such as `Sorted.empty()` needs one. This is slice 4 (#2766) of the
constrained-generics track (`design/tracks/constrained-generics.md`). The maintainer
approved #2766 as written: decisions A to I as recommended.

**Decision.**
1. **A static is generic over the type's parameters, then its own (A).**
   `fn Box.of(value: T) -> Box[T]` binds `T` from `type Box[T]`, as an instance method
   does, and a bound the type declares holds in the body with no `where` clause. The
   emitted shape does not change: a namespace member `Box.of<T>(value: T)`, the type's
   parameters first.
2. **Explicit type arguments (B).** `Box.empty[Int]()` lists the type's parameters,
   then the method's own, in declaration order, all or none; the arity is checked with
   the free-function code and message (`bynk.generics.type_arg_mismatch`). The order is
   the emitted `type_params ++ f.type_params` and the witness order fixed in slice 3, so
   one rule governs all three. `Box[Int].empty()` would need the reserved `name[T]`
   value form, a grammar decision with wider reach, and is not taken. The list is kept
   in the emitted call (`Box.empty<string>()`), so a call with no argument of type `T`
   still types as written. An instance method keeps rejecting explicit type arguments
   (F; ADR 0234 B).
3. **Inference (C).** First from the arguments, as for every generic call. Then, for a
   parameter still undetermined, by matching the declared return type structurally
   against the expected type, ignoring an expected type that is itself still being
   inferred. Otherwise `bynk.generics.uninferable_type_arg`. This applies to free
   generic calls, generic statics and generic instance methods, and not to capability
   operations, which keep their own explicit-or-inferred rule. It runs after the first
   (non-lambda) argument pass and before the lambda pass, so a lambda whose parameter
   type needs `T` can use a `T` the binding grounds. It fills only what the non-lambda
   arguments left open, so a program that compiled keeps its types and the goldens are
   byte-identical; one whose parameter only a lambda's return type would have fixed now
   grounds from the binding first, so its error can move into the lambda body. The call
   is still checked against the expected type afterwards. One rule
   answers the question for all three call paths: if `Box.empty()` grounded from its
   binding but `fn none[T]() -> Option[T]` did not, there would be two. Fixture
   `159_uninferable_type_arg` is rewritten to `let y = first_of(5)`, which has no
   expected type, so it keeps pinning the code. ADR 0174's ban on return-type overloading
   stands: this infers the parameters of one definition and does not choose between
   several by result type.
4. **Witnesses at static call sites (D).** `Callee::Static` carries a witness vector built
   by `check_declared_witness_args` from the solved substitution, in `witness_list`
   order, which already lists the receiver type's parameters first for any method.
   Lowering appends them after the ordinary arguments, as for an instance method, and a
   `Codable` witness's concrete types are codec roots. `Eq` and `Key` bounds are checked
   without a witness. Inside a generic caller a rigid parameter forwards its own witness.
5. **Statics are not values (E)** and **qualified variants are unchanged (I).** The
   type-argument gate opens only for a user static on a generic type, so `Opt.None[Int]`
   stays `bynk.generics.type_arg_mismatch`, and a qualified variant keeps grounding from
   its expected type.
6. **`method_on_generic_type` is narrowed, not retired (G, H).** It now fires for a static
   whose parameter and return types do not mention every parameter of its type
   (`fn Box.empty() -> Box[Int]`): nothing could determine the parameter at a call, every
   call would need an explicit argument that changes nothing, and a bounded type would
   demand a witness for a type only that argument names. The message names the missing
   parameter, with the note "make it a free function, or mention `T` in the signature". A
   `where` clause alone does not count as mentioning it.
7. **A spelling.** `from` is a reserved word, so the constructor the issue calls
   `Sorted.from` is written `Sorted.fromList`.

**Consequences.** ADR 0234 C and ADR 0183 D are lifted. A generic type can offer the
only way in to a bounded value. Every existing positive golden is byte-identical;
`367_method_on_generic_type` keeps its code with the new message, and
`159_uninferable_type_arg` is rewritten. Completion offers a user static after
`Type.`, generic or not. Explicit type arguments on an instance method, a codec for a
user generic type over a rigid parameter, and `Show` remain open.
