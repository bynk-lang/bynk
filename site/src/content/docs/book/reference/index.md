---
title: Reference
---
Consultable, complete, and dry. These pages describe exact behaviour; for
learning, start with the [tutorials](/book/tutorials/01-first-program/), and for
tasks see the [how-to guides](/book/guides/). For the toolchain itself — the CLIs,
the `bynk.toml` manifest, emission, and the editor tooling — see the
[Developer Documentation](/docs/).

## Language

- [Glossary](/book/reference/glossary/) — terse definitions of Bynk's load-bearing terms.
- [Type system](/book/reference/types/) — opaque, sum, record, and refined types.
- [Refined-type API](/book/reference/refined-types/) — `.of`, predicates, admission.
- [Operators & built-ins](/book/reference/operators/) — operators, precedence, built-in types.
- [Agents](/book/reference/agents/) — declaration, state, zeroability, lifecycle.
- [HTTP](/book/reference/http/) — HTTP handlers and `HttpResult`.
- [Static assets](/book/reference/assets/) — `asset` declarations: files served beside a Worker.
- [Embedded text](/book/reference/embeds/) — `embed` declarations: a project file's text as a `String`.
- [Testing](/book/reference/testing/) — `suite`/`case`, `expect`, `property`, `as <tier>`, `stub`, `Val[T]`.

## Project & output

- [Diagnostic index](/book/reference/diagnostics/) — every `bynk.*` code (generated).
- [TypeScript client](/book/reference/typescript-client/) — the typed client a non-Bynk Worker calls a context through, and `bynk.foreign.toml` pins.
- [Version compatibility & changelog](/book/reference/changelog/).

The `bynk.toml` manifest and [Emission](/docs/emission/) — the TypeScript each
construct compiles to — are documented under [Developer Documentation](/docs/).

## Generated reference

These pages are generated directly from the compiler (or the grammar) and
guarded by tests so they cannot drift:

- [Syntax & grammar](/book/reference/grammar/) — from the `tree-sitter-bynk` grammar.
- [Keywords](/book/reference/keywords/) — from the lexer's keyword tokens.
- [Diagnostic index](/book/reference/diagnostics/) — from the diagnostic registry.
- [Wire format](/book/reference/wire-format/) — the cross-context call format
  non-Bynk code can speak, from the emitter's own constants and codecs.

The [CLI (`bynkc`)](/docs/cli/) reference is also generated (from the clap command
tree); it now lives in [Developer Documentation](/docs/).
