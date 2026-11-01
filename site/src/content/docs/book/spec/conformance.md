---
title: "§10 Conformance & test corpus"
---
The conformance suite is the `bynkc` **fixture corpus**, together with the gates
that run over it ([§1.3](/book/spec/scope/)). This chapter defines what a conforming
implementation MUST do in terms of that corpus and those gates.

## §10.1 The fixture corpus

The corpus is two sets of fixtures:

- the **positive** suite (`bynkc/tests/fixtures/positive/`) — programs that MUST compile. Each is
  either single-file (an `input.bynk` compiled against an `expected.ts`) or a
  project (a `src/` tree compiled against a mirrored `expected/` tree). A
  conforming implementation MUST accept every positive fixture and emit the
  expected TypeScript.
- the **negative** suite (`bynkc/tests/fixtures/negative/`) — programs that MUST be rejected. Each
  pairs an input with an `expected_error.txt` naming the diagnostic categories the
  output MUST contain. A conforming implementation MUST reject every negative
  fixture, emitting a diagnostic of the specified `bynk.*` category
  ([§9](/book/spec/diagnostics/)).

Accepting every positive fixture and rejecting every negative one — with the
specified diagnostic — is the core of conformance.

## §10.2 The TypeScript gate

A conforming implementation's emitted output MUST be **type-correct end to end**:
for every project-form positive fixture, the emitted TypeScript MUST compile under
`tsc --strict` with no errors ([§8.4](/book/spec/compilation-model/#84-build-pipeline--conformance-to-typescript)).
This is the backstop for emission: a Bynk program's well-formedness is realised in
a type-checked TypeScript program, not merely asserted.

## §10.3 Runtime requirements

Beyond compiling and type-checking, the emitted program MUST *behave* as the
translation specifies. The following are normative requirements on the runtime
realised by the runtime library ([§7.4](/book/spec/runtime-library/)) and the emitted
modules:

- **Refinement validation runs at run time.** A refined or opaque `.of`
  constructor MUST test its predicate at run time, returning
  `Err(ValidationError)` for an input that violates the refinement and `Ok(value)`
  otherwise ([§7.3.1](/book/spec/emission/#731-types), [§7.4.2](/book/spec/runtime-library/#742-validationerror)).
- **Agent state persists within a run and resets between cases.** A committed
  agent state MUST be visible to later handler invocations addressing the same key
  within a run, and MUST be reset to its zero between test cases so each case
  starts from a clean slate.
- **Agent keys address state by value.** Semantically-equal keys MUST address the
  same state — records compared independent of field order
  ([§7.4.4](/book/spec/runtime-library/#744-agent-state)).
- **Tests execute and report.** `bynkc test` MUST compile the project, including
  the generated test modules, and run the aggregated runner on Node, reporting
  each case as pass or fail; an `assert` failure MUST be reported as a failing
  case with its source location ([§7.3.5](/book/spec/emission/#735-tests)).

> [!NOTE]
> Of these, the automated runtime gate executes the runtime library's agent
> helpers (key serialisation, state persistence, and reset) directly, and the
> TypeScript gate ([§10.2](#102-the-typescript-gate)) type-checks all emitted
> output; whole-program execution is exercised through the `bynkc test` pipeline.
> This note is informative.

## §10.4 Documentation gates

The specification's own examples are held to a related discipline, so the document
cannot misrepresent the compiler: every Bynk example in the Book is compiled by an
example gate, and every shown refusal is a real captured compiler transcript
rather than a paraphrase. These gates discipline the documentation, not the
language; this section is informative.
