---
title: "§9 Diagnostics"
---
The `bynk.*` diagnostic codes are the **identifiers of the static-semantics
rules** ([§5](/book/spec/static-semantics/)). Each well-formedness rule is named by the
code a conforming implementation emits when the rule is violated: when an
implementation rejects an ill-formed program, it MUST emit the code this
specification associates with the violated rule
([§1.3](/book/spec/scope/), [§5](/book/spec/static-semantics/)). The codes are stable identifiers
and are cited as such throughout §5.

The **complete catalogue** — every code, with a one-line summary and the
construct it governs, grouped by category — is the generated
[diagnostic index](/book/reference/diagnostics/). It is produced from the
compiler's diagnostic registry and drift-tested, so it cannot diverge from the
codes the compiler actually emits; this chapter does not restate it.

> [!NOTE]
> The catalogue is currently shared with the friendly reference, which is its
> present canonical home, exactly as the grammar appendix is
> ([§11](/book/spec/grammar-appendix/)). At the 1.0 reference-from-spec flip its canonical
> home may move under this specification. Either way it remains a single
> generated artefact. This note is informative.

## §9.1 Severity and the build gate

Each diagnostic carries a **severity** — `Error` or `Warning` — and the severity
decides whether it **fails compilation** (v0.89, ADR 0117):

- An **`Error`** is a well-formedness violation: the program is rejected and the
  compiler exits non-zero. No output is produced.
- A **`Warning`** is surfaced but does **not** fail the build: `bynk compile` and
  `bynk check` still **succeed (exit 0)** and emit their output, with the warnings
  reported alongside. A warning never gates emission.

A conforming implementation MUST classify each emitted diagnostic by the severity
this specification associates with its code, and MUST NOT fail compilation on a
`Warning`-severity diagnostic alone. The build-failure gate counts
`Error`-severity diagnostics only.

Warning-severity codes are marked **(warning)** in the
[diagnostic index](/book/reference/diagnostics/); examples are
`bynk.given.unused_capability`, `bynk.list.deprecated_function`, and the
`@indexed` hygiene codes (`bynk.index.missing` / `bynk.index.unused`). This
severity split is what lets a *deprecation* (`bynk.list.deprecated_function`) warn
rather than break a build. See [CLI exit codes](/docs/bynk-cli/).
