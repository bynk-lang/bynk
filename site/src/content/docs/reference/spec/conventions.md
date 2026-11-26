---
title: "§2 Notation & conventions"
---
This chapter fixes the notation and the conventions used throughout the
specification: how grammar productions are written, how well-formedness rules are
stated and tied to diagnostics, how dynamic meaning is specified, how normative
and informative content are distinguished, and how the specification is cited.

## §2.1 Grammar notation

Productions are written in EBNF:

`"x"` a literal token · `/x/` a regular expression · `( … )?` optional ·
`( … )*` zero or more · `( … )+` one or more · `a | b` choice · `ε` empty.

**Nonterminals** are the unquoted names. Each name is the *display* name of a
grammar rule: a leading underscore (an internal helper rule) is dropped and
trivial wrappers are collapsed, so a production reads as a language rule rather
than a parser internal. Every production shown in this specification is
**generated** from the `tree-sitter-bynk` grammar, so it cannot drift from that
grammar.

> [!NOTE]
> The `tree-sitter-bynk` grammar is the parser used by the editor tooling; it is
> a separate implementation from the recursive-descent parser the compiler runs.
> A [conformance test](https://github.com/bynk-lang/bynk/blob/main/tree-sitter-bynk/tests/conformance.rs)
> parses the same sources with both and asserts they accept and reject the same
> programs, so the generated productions describe what the compiler accepts, not
> only what the editor grammar does.

The complete, byte-exact grammar — every production in one block — is the grammar
appendix (§11).

## §2.2 How syntax is shown

A production is embedded by name. For example, the HTTP route handler:

{{#grammar http_handler}}

A production states what *parses*. It does not, by itself, state what is legal
beyond parsing; that is the role of the well-formedness rules in §2.3.

## §2.3 How rules are written

**Static semantics are well-formedness rules.** A program is well-formed exactly
when a conforming implementation accepts it (see [§1.3](/reference/spec/scope/)). Each rule is
stated normatively and is tied to the `bynk.*` diagnostic code(s) a conforming
implementation MUST emit when the rule is violated. The mapping is generated from
the compiler's diagnostic registry, so a rule and its governing diagnostics
cannot drift apart.

A rule's governing diagnostics are embedded directly beneath it. For the HTTP
route handler of §2.2, the diagnostics that constrain it beyond parsing are:

{{#grammar-semantics http_handler}}

Each entry links by code to the [diagnostic index](/reference/diagnostics/),
which is the normative diagnostics catalogue (§9). A construct with no governing
diagnostics says so rather than omitting the block — an unconstrained production
is a legitimate state, not an oversight.

## §2.4 How meaning is specified

Dynamic meaning is **translation-defined**. The specification does not give an
operational or evaluation semantics; instead, the behaviour of each construct is
defined by:

1. the TypeScript that construct **emits** — specified per construct in the
   [emission chapter (§7)](/reference/spec/emission/), for both compilation targets; and
2. the **runtime-library contract** the emitted code runs against — the normative
   [runtime library (§7.4)](/reference/spec/runtime-library/).

A program's meaning is therefore the meaning of its emitted TypeScript executed
against that runtime contract. This is the whole of Bynk's dynamic semantics:
there is no separate model to consult. The [emission reference](/reference/tooling/emission/)
gives the friendly view of the same emission.

## §2.5 Normative and informative content

Prose in this specification is **normative by default**: it states requirements,
using the RFC 2119 keywords of [§1.2](/reference/spec/scope/) where a requirement is binding.

**Informative** content — explanation, motivation, and examples — is marked as
such and imposes no requirement:

> [!NOTE]
> A note in this form is informative. It clarifies or motivates, but a conforming
> implementation is bound only by the normative prose, never by a note.

**Examples are informative.** A Bynk example illustrates a rule; it never extends
or overrides it. Examples are nonetheless held honest by the documentation's
example gate (every example compiles, and every shown refusal is a real captured
transcript) — an example may be informative, but it cannot misrepresent what the
compiler does.

## §2.6 Citation and language

Chapters and their sections are numbered `§X.Y` (for example, this section is
§2.6); cite the specification by that number, which is stable across edits.
Appendices are lettered (Appendix A, Appendix B).

The specification is written in **British English**.
