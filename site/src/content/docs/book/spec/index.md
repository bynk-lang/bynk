---
title: The Bynk Language Specification
---
The normative definition of Bynk: the language as accepted and compiled by
`bynkc` at the **current version, v0.419**. It states what a conforming
implementation must accept, what it must reject, and what a program means. Where
the [grammar reference](/book/reference/grammar/) is a friendly, per-construct
lookup for people writing Bynk, this is the complete, citable definition for
implementers and for precise reference. Each language increment updates this
document in place ([Scope §1.1](/book/spec/scope/)); the decisions behind increments are
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

[Conventions §2](/book/spec/conventions/) makes this model precise and fixes the
notation; [Scope §1](/book/spec/scope/) fixes what is normative and what conformance
means.

## Chapters

- [§1 Scope & conformance](/book/spec/scope/) — what the spec covers; what a conforming
  implementation must accept and reject; RFC 2119 keywords.
- [§2 Notation & conventions](/book/spec/conventions/) — grammar notation; how rules are
  written and linked to diagnostics; the translation-defined model; normative vs
  informative; citation.
- [§3 Lexical grammar](/book/spec/lexical-grammar/) — tokens, identifiers, literals,
  comments, doc-blocks, trivia.
- [§4 Syntactic grammar](/book/spec/syntactic-grammar/) — the productions, organised by
  construct.
- [§5 Static semantics](/book/spec/static-semantics/) — well-formedness per construct,
  woven to diagnostics.
- [§6 The type system](/book/spec/type-system/) — base, refined, opaque, sum, record, and
  enum types; `Result`, `Option`, `Effect`; refinement and admission.
- [§7 Meaning by translation](/book/spec/emission/) — what each construct emits, and the
  [runtime-library contract](/book/spec/runtime-library/).
- [§8 Compilation model](/book/spec/compilation-model/) — the `bynk.toml` manifest, project
  layout, and the build contract.
- [§9 Diagnostics](/book/spec/diagnostics/) — the normative catalogue; the codes are the
  identifiers of the §5 rules.
- [§10 Conformance & test corpus](/book/spec/conformance/) — the `bynkc` fixture corpus as
  the conformance suite.
- [§11 Complete grammar](/book/spec/grammar-appendix/) — the complete generated grammar.
- [Appendix A — Planned features](/book/spec/appendix-planned/) (post-MVP, non-normative).
- [Appendix B — Version history](/book/spec/appendix-version-history/).
