---
title: The Bynk Language Specification
---
The normative definition of Bynk: the language as accepted and compiled by
`bynk` at the **current version, v0.681**. It states what a conforming
implementation must accept, what it must reject, and what a program means. Where
the [grammar reference](/reference/grammar/) is a friendly, per-construct
lookup for people writing Bynk, this is the complete, citable definition for
implementers and for precise reference. Each language increment updates this
document in place ([Scope §1.1](/reference/spec/scope/)); the decisions behind increments are
recorded in `design/decisions/`.

The two coexist by register, not by contradiction. They draw on the **same
generated ingredients** — the grammar productions, the static-semantics-to-
diagnostics weave, the diagnostic index, the grammar appendix — so the hard
facts cannot diverge; only the prose differs (explanatory there, normative here).

> [!NOTE]
> This section is built phase by phase. The chapters listed below as plain text
> are not yet written; they are the planned structure, shown so the shape of the
> whole is visible. Only written chapters appear in the navigation. This is
> informative.

## How meaning is defined

Bynk is **translation-defined**. Its three layers of definition are:

- **Syntax** — the grammar, generated from `tree-sitter-bynk`, so the
  productions in this spec cannot drift from that grammar; a cross-parser
  conformance test keeps it in agreement with the compiler's own parser.
- **Static semantics** — well-formedness rules. A program is well-formed exactly
  when it provokes no `bynk.*` diagnostic; each rule is tied to its diagnostic
  code(s), so the rule catalogue and the compiler cannot drift.
- **Dynamic meaning** — defined **by translation**: each construct's behaviour is
  the TypeScript it emits, together with the runtime-library contract. There is
  no separate operational semantics.

[Conventions §2](/reference/spec/conventions/) makes this model precise and fixes the
notation; [Scope §1](/reference/spec/scope/) fixes what is normative and what conformance
means.

## Chapters

- [§1 Scope & conformance](/reference/spec/scope/) — what the spec covers; what a conforming
  implementation must accept and reject; RFC 2119 keywords.
- [§2 Notation & conventions](/reference/spec/conventions/) — grammar notation; how rules are
  written and linked to diagnostics; the translation-defined model; normative vs
  informative; citation.
- [§3 Lexical grammar](/reference/spec/lexical-grammar/) — tokens, identifiers, literals,
  comments, doc-blocks, trivia.
- [§4 Syntactic grammar](/reference/spec/syntactic-grammar/) — the productions, organised by
  construct.
- [§5 Static semantics](/reference/spec/static-semantics/) — well-formedness per construct,
  woven to diagnostics.
- [§6 The type system](/reference/spec/type-system/) — base, refined, opaque, sum, record, and
  enum types; `Result`, `Option`, `Effect`; refinement and admission.
- [§7 Meaning by translation](/reference/spec/emission/) — what each construct emits, and the
  [runtime-library contract](/reference/spec/runtime-library/).
- [§8 Compilation model](/reference/spec/compilation-model/) — the `bynk.toml` manifest, project
  layout, and the build contract.
- [§9 Diagnostics](/reference/spec/diagnostics/) — the normative catalogue; the codes are the
  identifiers of the §5 rules.
- [§10 Conformance & test corpus](/reference/spec/conformance/) — the `bynk` fixture corpus as
  the conformance suite.
- [§11 Complete grammar](/reference/spec/grammar-appendix/) — the complete generated grammar.
- [Appendix A — Planned features](/reference/spec/appendix-planned/) (post-MVP, non-normative).
- [Appendix B — Version history](/reference/spec/appendix-version-history/).
