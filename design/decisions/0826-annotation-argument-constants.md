# 0826 — A constant in a literal-only annotation argument

- **Status:** Accepted (v0.575)

**Context.** ADR 0678 C kept a constant out of compile-time positions because "that needs evaluation at check time". ADR 0762 showed that for a literal-only position it does not: a constant's initialiser is the closed static set (ADR 0678 A: no operators, calls or interpolation, cycles refused), so a reference resolves, by following constant-to-constant references, to the literal text in the source. A wire name is the case that matters, since two places must agree on it (`@name("__Host-visits")` on a cookie record's field and the cookie a handler writes) and the language gave no way to say so once (#3445).

**Decision.**

- **A. Rule.** A bare reference to a constant counts as the literal it is bound to wherever an annotation argument takes a literal of that type. The constant's declared type must be exactly the base type (a refined or opaque constant does not count); each hop of a constant-to-constant chain is held to it. Constants from a used commons or a sibling file are in scope by the usual rule, and a `private` constant does not leave its unit. Actor scheme arguments and refinement bounds are not annotation arguments and are unchanged (ADR 0678 C's refinement-bound half stands).
- **B. One pass, ahead of the readers.** After a unit's constants are known and before any annotation argument is read, `annotation_consts::resolve_annotation_consts` replaces each such argument's value with the literal it resolves to. The validators and the syntactic readers (`wire_name`, the OpenAPI readers, hover, the emitters) hold no constant table, so resolving once upstream means none can miss it: a reader that did would silently fall back to the field's own name. The argument keeps the reference's span and records the constant (`AnnotationArg::from_const`).
- **C. One implementation.** `consts::chain_literal` is the resolution loop; ADR 0762's `sql_literal_text` and the imported-alias fold of ADR 0762 now call it, pinned by their existing fixtures.
- **D. No new diagnostic.** Validation runs on the resolved text under the annotation's existing code. A diagnostic points at the reference, names the constant and, where it is declared in the same file, carries a related span there. An argument that does not resolve is left as written, so the annotation's existing bad-argument diagnostic fires, with a note naming the constant and its declared type.
- **E. IDE.** The reference is recorded as a use of the constant, so go-to-definition, hover, find-references and rename see it.
- **F. This increment.** The pass and `@name` (ADR 0812 C, "exactly one string literal", is read as "a string literal or a `String` constant bound to one"). The remaining value-taking annotations follow in the next increment, which adds rows to the pass's rule table.

**Consequences.** A wire name is written once. Every program that compiles today compiles and emits identically: the change only widens what is accepted, and the one observable difference is the wording of `bynk.types.field_name_bad_arg`. No grammar, emitter or runtime change.
