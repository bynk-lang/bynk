# 0760 — An `@example` annotation gives Val a value for a Matches or IsoDate refinement

- **Status:** Accepted (v0.523)

**Context.** `Val` cannot invent a string for an arbitrary regex, so bare
`Val[T]` of a `Matches` (or `IsoDate`) type, or of a record with such a field,
reported `bynk.val.needs_pin` (ADR 0148, 0149 D2). The derived smoke (ADR 0677)
writes `Val[T]` for every argument itself, so every route with a patterned path
parameter or body field lost its *answers* case. Proposed in #2686.

**Decision.**

- **A. Syntax.** An `@example(<literal>)` annotation after the refinement,
  reusing the `@name(args)` grammar (ADR 0111). Rejected: a second argument on
  `Matches` (its string payload is read by contracts, the TypeScript client and
  OpenAPI, where an example must not matter), a doc-comment tag (a comment
  would decide a compile result), and a new keyword.
- **B. Placement.** A type declaration's refinement and a record field's inline
  `where`, each carrying at most one. The annotations live on `Refinement`, so
  they are not predicates and never reach contracts, the wire format or emitted
  code.
- **C. What may carry one.** Only a refinement `Val` cannot generate
  (`Matches`, `IsoDate`); another is `bynk.types.example_unneeded`. Widening
  later is additive; narrowing would break users.
- **D. Verification.** At the declaration, whether or not a test uses it: one
  literal of the base type (`bynk.types.example_not_literal`) satisfying every
  predicate (`bynk.types.example_violates`), using the evaluator `Val[T](lit)`
  already uses (`regress`, ECMAScript semantics, as the emitted `RegExp`). Also
  `bynk.types.duplicate_example` and `bynk.types.unknown_annotation`.
- **E. `for all` and `History` are unchanged.** An example is one value, so a
  property over it would run one case under a count of many, which misreports
  coverage. They still raise `bynk.val.needs_pin`.
- **B2. Named-type fields.** An `@example` on a field whose type is a named type
  is refused (`bynk.types.example_on_named_field`): the example belongs on the
  type. A field that narrows a named type needing a pin with extra inline
  predicates still needs a pin, because the type's example was checked only
  against the type's own predicates.
- **F. Where it is read.** Bare `Val` only (also through a record or sum), and a
  pin overrides it. Not the contract, OpenAPI or the emitted program.

- **G. Tooling (ADR 0156).** Hover: on `@example` it shows the example and the
  refinement it satisfies. Completion: `@` (now a trigger character) after a
  `Matches(…)`/`IsoDate` refinement offers `example`, and only there. Semantic
  tokens: `@example` is a `decorator`, as handler annotations are. Signature
  help: unchanged, because `@example(...)` is an annotation, not a call.

**Consequences.** `bynk test --smoke` runs *answers* for a route whose `Matches`
types carry examples. A stale example (the pattern was tightened) is a compile
error at its declaration. The example is branded without a runtime check, so a
divergence between `regress` and V8 would surface as the route rejecting the
smoke's own request; the smoke fixture exercises that path on every run.
Generators drawing from several examples, and an OpenAPI `example` field, are
follow-ups this leaves room for.
