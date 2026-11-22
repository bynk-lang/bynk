---
title: Reference
description: Exact behaviour of the Bynk language and toolchain — the language, its architecture, adapters, the tools, every diagnostic, the normative specification and every release.
---
Consultable, complete, and dry. These pages describe exact behaviour; for
learning, start with the [tutorials](/book/tutorials/01-first-program/), and for
tasks see the [how-to guides](/book/guides/). The Reference has seven sections.

## Language

What each construct means and how its types behave.

- [Syntax & grammar](/reference/grammar/) and [Keywords](/reference/keywords/) — generated from the grammar and the lexer.
- [Glossary](/reference/glossary/) — terse definitions of Bynk's load-bearing terms.
- [Type system](/reference/types/) — opaque, sum, record, and refined types.
- [Generics and bounds](/reference/generics/) — type parameters, `T: Eq`, `T: Ord`, `T: Key`, a method's `where` clause.
- [Refined-type API](/reference/refined-types/) — `.of`, predicates, admission.
- [Operators & built-ins](/reference/operators/) — operators, precedence, built-in types.
- [Agents](/reference/agents/) — declaration, state, zeroability, lifecycle.
- [HTTP](/reference/http/) — HTTP handlers and `HttpResult`.
- [Static assets](/reference/assets/), [Embedded text](/reference/embeds/), [Test fixtures](/reference/fixtures/) and [Named constants](/reference/constants/).
- [Testing](/reference/testing/) — `suite`/`case`, `expect`, `property`, `as <tier>`, `stub`, `Val[T]`.

## Architecture

What a project compiles to, and how its parts talk to each other and to code
that isn't Bynk.

- [Emission](/reference/tooling/emission/) — the TypeScript each construct compiles to.
- [Wire format](/reference/wire-format/) — the cross-context call format non-Bynk code can speak, generated from the emitter's own constants and codecs.
- [TypeScript client](/reference/typescript-client/) — the typed client a non-Bynk Worker calls a context through.
- [HTTP client](/reference/http-client/) — the typed client `bynk client` generates for a front end.
- [Registry protocol](/reference/tooling/registry-protocol/) — how packages are published and resolved.

## Adapters

- [Adapters](/reference/adapters/) — wrapping a TypeScript library behind a capability.

## Tooling

- [Tooling](/reference/tooling/) — the `bynk` CLI, the `bynk.toml` manifest, editor support, and a page for each tool.

## Diagnostics

- [Diagnostic index](/reference/diagnostics/) — every `bynk.*` code, generated from the compiler's registry, each with its own page.

## Specification

- [The Bynk Language Specification](/reference/spec/) — the normative definition of the language, kept whole.

## Releases

- [Releases](/reference/releases/) — every release, newest first, and the increments each one shipped.
- [Upgrading](/reference/upgrading/) — what to change when you move to a newer release.
