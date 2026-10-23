---
title: "§1 Scope & conformance"
---
## §1.1 Scope

This specification defines the Bynk language as accepted and compiled by `bynkc`
at the **current version, v0.398**. It is maintained as the single source of
truth: each language increment updates the affected chapters of this document,
so its scope is always the shipped language. It is normative for shipped
behaviour: where this document and the compiler disagree about a program that
falls within this scope, that is a defect in one of them, to be reconciled.

It covers the language's syntax (the grammar), its static semantics (the
well-formedness rules a program MUST satisfy), and its dynamic meaning (defined
by translation to TypeScript together with the runtime-library contract). It
does not specify the compiler's internals, its command-line surface beyond the
build contract, or any particular editor tooling.

> [!NOTE]
> Planned-but-unshipped features — sagas, event replay, and the other directions
> the appendix names — are **out of scope** and are not part of the normative language. They are
> sketched in a planned-features appendix purely to record design intent. The
> design decisions behind shipped increments are recorded separately in
> `design/decisions/`. Nothing in this paragraph is normative.

## §1.2 Conformance language

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**,
**SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this
specification are to be interpreted as described in RFC 2119. In brief:

- **MUST** / **REQUIRED** / **SHALL** — an absolute requirement.
- **MUST NOT** / **SHALL NOT** — an absolute prohibition.
- **SHOULD** / **RECOMMENDED** — a requirement that may be waived only with full
  understanding of the consequences.
- **MAY** / **OPTIONAL** — genuinely discretionary.

These keywords carry their RFC 2119 meaning only in normative prose (see
[§2.5](/book/spec/conventions/)); in an informative note or example they are ordinary
English.

## §1.3 Conformance

A **conforming implementation** of Bynk MUST, for every program within the scope
of [§1.1](#11-scope):

- **accept** every program in the positive conformance suite, compiling it
  without error; and
- **reject** every program in the negative conformance suite, emitting the
  diagnostic(s) this specification associates with the violated well-formedness
  rule (see [§2.2](/book/spec/conventions/) and the diagnostics catalogue).

A program is **well-formed** exactly when a conforming implementation accepts it.
The static-semantics rules (the §5 chapter) state the conditions for
well-formedness; each is tied to the `bynk.*` diagnostic code that a conforming
implementation MUST emit when the condition is violated.

The **conformance suite is the `bynkc` fixture corpus** — its positive fixtures
(which MUST compile) and its negative fixtures (which MUST fail, with the stated
diagnostic). The gates that enforce this, and the corpus's role as the
authoritative suite, are detailed in the later Conformance & test corpus chapter
(§10).

> [!NOTE]
> This document's own examples are held to the same standard: every Bynk example
> in the specification is compiled by the documentation's example gate, and every
> shown refusal is a real, captured compiler transcript. An example is
> informative, but it cannot lie about what compiles.

## §1.4 See also

For the explanatory, per-construct view of the same language — productions with
prose and examples, aimed at people writing Bynk — see the friendly
[grammar reference](/book/reference/grammar/). It and this specification share
their generated facts; they differ only in register.
