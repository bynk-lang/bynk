# 0281 — A capability operation may declare its own type parameter

- **Status:** Accepted (v0.235)

**Context.** A capability interface method could not declare its own type
parameter: `capability_op` (`tree-sitter-bynk/grammar.js`) was a fixed
`"fn" name "(" params ")" "->" return_type`, unlike a qualified static call,
which has supported explicit type arguments since v0.22b (`Json.decode[T](s)`,
ADR 0045). The Idempotency capability track
([design/archive/retired-tracks.md](../archive/retired-tracks.md)
§3.1) wants `fn dedup[T](…) -> Effect[Option[T]]` — a dedup check that hands
back a cached value of whatever type the *calling handler's own return type*
is, which the capability's author cannot know ahead of time. Capability calls
dispatch through a constructor-injected provider instance (ADR 0005) and Bynk
has no first-class generic function values, so there is no userland
workaround — the declaration syntax itself has to support it. Two existing
generics slices are not this shape: generic records (ADR 0183) and generic
instance methods (#594) both carry the parameter on a **type**; this is the
first user-declared surface where the parameter belongs to the **method
itself**, independent of any enclosing type — the shape `Json.decode[T]` has,
but `Json.decode` is a compiler-backed static on a built-in module, not a
provider-dispatched `given`-capability method.

**Decision.** `capability X { fn op[T](…) -> … }` parses, checks, and emits,
with `T` resolved only from an explicit call-site type argument
(`X.op[Some](…)`) — never inferred.

- **A. Grammar shape mirrors `fn_decl`/`type_decl`'s own `[A, B]` list.**
  `capability_op` gains the identical
  `optional(seq("[", sep1(type_param, ","), "]"))` snippet already inlined at
  both declaration sites — there is no shared named rule to reuse, just a
  copy-paste convention already established twice.
- **B. Explicit only, no inference.** Same restriction `Json.decode[T]` lives
  under, minus even its `expected`-type fallback — a capability op can carry
  more than one type parameter, so it reuses the arity-check-then-substitute
  half of the free-generic-function machinery (`check_generic_call`), not its
  two-pass unification.
- **C. A genuine generic TS interface method — no monomorphisation, no
  erasure.** `dedup<T>(key: string): Promise<Option<T>>` is emitted directly
  (`ts_type_params`, the same helper a generic record's own methods use). This
  is the "honest passthrough" default the rest of the capability-provider
  machinery already follows, and it forced a second departure from
  `Json.decode[T]`'s precedent: that call site specialises a runtime codec per
  call and needs no TS-level generic at all, whereas a capability op's `T` is
  a pure type-level parameter with no runtime codec to specialise — nothing
  at the call site is generic *except* the type, so `tsc` cannot infer a
  return-position-only parameter from nothing. The call site therefore renders
  the source-level explicit type argument as an explicit TS generic argument
  (`deps.Cap.op<T>(…)`), reusing the AST's own `TypeRef` rather than
  round-tripping through the checker's resolved `Ty`.
- **D. No codec/persistence story.** Serialising or persisting a value of an
  unconstrained `T` is out of scope — that belongs to Idempotency's eventual
  durable-provider slice, not this grammar/checker/emitter mechanism.
- **E. A generic capability operation requires an external (bodiless)
  provider.** A Bynk-bodied provider would need `T` rigid through
  `check_handler_body` for a body that, being unable to construct an arbitrary
  `T`, can only ever return a value it received (a parameter) or `None` —
  expressive gain that does not justify threading generics through the
  handler-body checker and the provider/capability signature-match check's
  type-parameter-aware renaming. External providers already skip signature
  matching entirely (a hand-authored TS class, checked by `tsc`), so this
  costs no new grammar on `provider_op`: `bynk.provider.generic_op_requires_external`
  rejects a Bynk-bodied `provides` for any capability whose op is generic.
  Consequence: a capability with a generic op can only ever be declared inside
  an `adapter` (the only place an external provider is legal) — never a plain
  `context`, whose provider must have a Bynk body.
- **F. Stubbing a generic operation is deferred, not supported.** A test's
  `__Stub_Cap` class has no way to construct a value of an unconstrained `T`.
  Deferred cleanly: the stub class carries no `implements` clause and is wired
  through an untyped `deps` seam, so `bynk.stub.generic_op` rejects stubbing
  the generic op while stubbing any other op on the same capability keeps
  type-checking.
- **G. Both cross-context capability-call forms are in scope.** A flattened
  call (`Cap.op[T](…)` via `consumes Adapter { Cap }`) and a qualified call
  (`Adapter.Cap.op[T](…)` via `consumes Adapter`) thread the same
  arity-check/substitution through `check_static_call` and
  `check_cross_context_capability_call` respectively — the qualified form
  costs the same handful of lines as the local path, and leaving it
  unsupported would be an undocumented asymmetry on the same adapter
  capability.

**Consequences.** `capability Idempotency { fn dedup[T](key: String) ->
Effect[Option[T]] }`, implemented only by an external provider and called as
`Idempotency.dedup[SomeType](key)`, is the first slice-0 building block the
Idempotency track's §3.1 needs — §3.1's own open questions (how the dedup
short-circuit interacts with the rest of a handler body, provider-variant
selection, the durable-provider's atomic-commit story) remain unresolved and
are explicitly out of scope here; this ADR is the grammar/checker/emitter
mechanism alone, not an `Idempotency` capability. A context-rebranded commons
type instantiating `T` at a flattened cross-context call site (the class of
gap ADR 0256 found for Locale) is covered by a dedicated fixture: since the
type argument only *names* a type rather than constructing one, no rebranding
hazard exists — the call is a pure generic instantiation. `CapabilityOpInfo`
and `CrossContextCapabilityOp` (`bynk-check`) both gain a `type_params` field,
and the capability-info construction sites that used to fully resolve a
signature to ground `Ty` now resolve it with the op's own vars in scope
(`resolve_type_ref_in`), so a declared `T` survives as `Ty::Var` for a call
site to substitute. Hover, signature help, and completion render the `[T]`
slot (ADR 0156); the formatter round-trips it. Both remaining deferred
surfaces (D/E/F above) are named follow-ons, not silent gaps.
