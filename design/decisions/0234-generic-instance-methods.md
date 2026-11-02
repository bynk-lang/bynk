# 0234 — Instance methods on generic types

- **Status:** Accepted (v0.209)

**Context.** The generic-record increment (ADR 0183, #546) let an author name a
generic data shape (`type Box[A] = { value: A }`) but rejected attaching a method
to it (`bynk.generics.method_on_generic_type`): a method on `Box[A]` needs
generic methods, which ADR 0028 keeps additive, and a naïve emission produces an
under-applied `self: Box` TypeScript signature. So a generic type had no
`value.method(…)` surface; authors had to write free functions taking the generic
value. Follow-up #594 asked for the missing surface. The design question is the
type-parameter scoping (the type's parameters plus the method's own) and the
emitted namespace/`self` shape — the checker's argument-directed inference
(ADR 0029) already covers the method-body side.

**Decision.**

**[DECISION A] An instance method on a generic type is a generic method.** A
method whose receiver is a generic type sees the receiver type's parameters in
scope, as rigid type variables, alongside any it declares itself:
`fn Box.map[U](self, f: A -> U) -> Box[U]` reads `A` from the receiver `Box[A]`
and binds its own `U`. `self` is typed as the receiver applied to its own
parameters (`Box[A]`), so field access substitutes them exactly as a generic
record's fields do. A method type parameter may not reuse one of the receiver
type's parameter names — the two would collapse in the substitution — and is
rejected with `bynk.generics.duplicate_type_param`.

**[DECISION B] The call site infers the type arguments; emission is erased.** A
call `value.map(f)` resolves against a substitution seeded from the receiver's
concrete type arguments (the type's parameters are ground the moment the
receiver's type is known), then infers the method's own parameters from the
argument types by the same argument-directed unification as a generic function
call (ADR 0029). Emission threads the type's parameters onto *each* method of the
namespace object — `map<A, U>(self: Box<A>, f: (a: A) => U): Box<U>` — because the
namespace `const` is a value and cannot itself carry `<A>`. `tsc` enforces the
arguments; no per-instantiation TypeScript is generated, matching generic-record
and generic-function erasure. The explicit `value.map[U](…)` call form is
deferred (type arguments are inferred), as it was for the bare-generic-value form
in ADR 0029.

**[DECISION C] Static methods on a generic type stay deferred.** A static method
(`fn Box.of(value) -> Box[A]`, no `self`) has no receiver to supply the type's
parameters, so it would need free-function-style inference of the type's own
parameters — a separable follow-on. It keeps
`bynk.generics.method_on_generic_type` (now narrowed to the static case). Instance
methods are the surface #594 asked for and the narrowest coherent lift.
_(Lifted by the constrained-generics slice 4 ADR: a static is generic over the
type's parameters, inferred from its arguments, the expected type or explicit type
arguments.)_

**Consequences.**

- A generic record's construction inference now binds a type parameter from an
  actual that is ground *up to the enclosing method's rigid variables*
  (`Box { value: <U> }` inside `Box.map[U]` infers `Box[U]`), where before it
  required a fully ground actual. Outside a generic body the enclosing rigid set
  is empty, so the rule is unchanged.
- Bounds, generic sums, static methods on generic types, higher-kinded
  parameters, and the explicit `value.map[U](…)` call form all stay reachable and
  unshipped — the ADR 0028 "nothing ships that would have to be unshipped"
  property is preserved.
- The narrowest lift that answers #594: a generic type gets a method surface;
  static constructors and the explicit-type-argument form are well-scoped
  follow-ons.
