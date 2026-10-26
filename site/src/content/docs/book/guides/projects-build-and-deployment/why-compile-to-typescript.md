---
title: Why compile to TypeScript
---
Bynk does not have its own runtime. It compiles to TypeScript and runs on the
JavaScript platform — specifically targeting Cloudflare Workers. This page
explains what that choice buys and what it costs.

## What it buys

**Reach without a new ecosystem.** A language is only as useful as the libraries,
tools, and platforms it can use. By emitting TypeScript, Bynk inherits npm, the
JavaScript tooling world, and a mature, globally distributed serverless runtime
on day one, instead of asking anyone to adopt a bespoke VM.

**Readable, debuggable output.** The generated TypeScript is meant to be read.
Every `commons`, type, and handler maps to recognisable TypeScript — branded
types, discriminated unions, plain functions. When something misbehaves you can
open the emitted code and see exactly what runs; there is no opaque bytecode in
between. The compiler also emits a source map (`<file>.ts.map`) alongside each
generated `.ts`, so stack traces and a debugger resolve back to the original
`.bynk` lines; the `.bynk` source is embedded for local builds and kept out of
deployed Workers by default. In VS Code, **Bynk: Show Compiled Output** opens a
file's generated TypeScript beside it, with the cursor in either editor
highlighting the matching lines in the other, so you need not build to look.

**A deployment model that matches the language model.** Bynk's units line up with
the platform's units: a context becomes a Worker, and a stateful agent becomes a
Durable Object. The architecture you express in the language is the architecture
you deploy, with no impedance mismatch to paper over.

**A type system in front of a dynamic runtime.** JavaScript is permissive; Bynk
puts a strict, refinement-aware type system ahead of it, then emits TypeScript
whose own types reinforce the guarantees. You get the platform's flexibility with
a much stronger static contract.

## What it costs

**You inherit JavaScript's runtime semantics.** Numbers are IEEE-754 doubles;
`Int` is `number`. Bynk's types prevent many mistakes, but the underlying value
domain is still JavaScript's, and that occasionally shows through.

**Boundaries need validation.** Data arriving from the network — an HTTP body, a
cross-context call on the `workers` target — is untyped JSON until proven
otherwise. Bynk generates validation at those boundaries, which is correct but is
work the runtime does that a single-process typed language might avoid.

**Two targets to reason about.** The same Bynk program emits differently for
`bundle` (direct calls) and `workers` (calls over Service Bindings). The source is
identical, but the operational characteristics differ, and you have to know which
you are building. See [Target Cloudflare Workers](/book/guides/projects-build-and-deployment/cloudflare-workers/).

## The bet

The wager is that for the services Bynk targets, *reach and a proven runtime* are
worth more than *control over a bespoke runtime* — and that a strong type system
plus readable output recovers most of what you might otherwise miss. See
[Why Bynk exists](/book/about/why-bynk-exists/) for the wider motivation.
