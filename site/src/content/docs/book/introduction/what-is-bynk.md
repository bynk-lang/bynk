---
title: What is Bynk?
---
Bynk is a statically typed programming language for building services. It is
**architecture-first**: the shape of a program — its
[contexts](/reference/glossary/#term-context),
[services](/reference/glossary/#term-service),
[agents](/reference/glossary/#term-agent), and the types that flow between
them — is part of the language, not a convention layered on top. Bynk compiles to **typed TypeScript** and targets
**Cloudflare Workers**.

## The idea in one example

A minimal Bynk program declares a context and a type:

```bynk
commons demo {
	type Id = Int
}
```

A small HTTP service is not much larger:

```bynk
context greet

service api from http {
	on GET("/ping") () -> Effect[HttpResult[String]] by Visitor {
		Ok("pong")
	}
}
```

Compiling either with `bynk` produces TypeScript you can read, run, and deploy.
To see all of this wired together in one complete program — types, a context, a
capability, a stateful agent, and an HTTP service — read
[Anatomy of a Bynk service](/book/introduction/anatomy-of-a-service/).

## Why Bynk exists

Bynk makes three bets about what goes wrong in services, and how a language can
help.

### The problem: services drift from their design

Most services begin with a clear architecture in someone's head: these are the
boundaries, this owns that state, this calls that. Little of it survives
contact with the code. The architecture lives in diagrams and tribal knowledge,
and the code *implements* the design without ever *stating* it. Over time the
two drift apart, and the gap is where bugs and onboarding pain live.

So **the architecture is in the language**. A
[context](/reference/glossary/#term-context) is a deployable boundary, a
[service](/reference/glossary/#term-service) groups handlers, and an
[agent](/reference/glossary/#term-agent) is a named, keyed owner of state.
They are not naming conventions or folder layouts: the compiler understands and
enforces them, so the shape of the system is written down and checked, and
can't quietly rot. See
[How a Bynk program is shaped](/book/concepts/program-structure/how-a-program-is-shaped/).

### The bet: make illegal states unrepresentable

Many runtime errors are really *type* errors the type system was too weak to
catch. An order id is "just a string", so it gets swapped with a customer id. A
percentage is "just a number", so one day it is `240`. Bynk closes these gaps:

- **[Refined types](/reference/glossary/#term-refined-type)** carry a
  predicate, so `Age = Int where InRange(0, 150)` is a distinct type whose
  invalid values *can't be constructed*.
- **[Opaque types](/reference/glossary/#term-opaque-type)** give a value a
  nominal identity, so an `OrderId` and a `CustomerId` never mix.
- **Errors are values.** An operation that can fail returns a
  [`Result`](/reference/glossary/#term-result-option), and absence is an
  `Option`. There are no exceptions to forget to catch.
- **Agent state must be [zeroable](/reference/glossary/#term-zeroable)**, so a
  never-seen key has a well-defined starting state.

The aim is to make the wrong program fail to build. In TypeScript, the
impossible percentage compiles:

```typescript
const age: number = 240; // compiles — 240 is a perfectly good `number`
```

In Bynk, it doesn't:

```bynk,fail
{{#include ../../diagnostics/refine_out_of_range.bynk}}
```

```text
{{#include ../../diagnostics/refine_out_of_range.txt}}
```

The diagnostic names the invariant you broke: when Bynk declines a program, the
refusal is the lesson. Bynk is a service-tier language, not a teaching tool — but
a language that explains every refusal is one you come to understand *as you
use it*. See
[The type-system philosophy](/book/concepts/type-system/philosophy/) and
[The agent model](/book/concepts/agents-and-state/the-agent-model/).

### The pragmatic choice: compile to typed TypeScript

A new language with a new runtime asks the world to adopt an ecosystem before
it is useful. Bynk instead **compiles to typed TypeScript** and targets
**Cloudflare Workers**:

- the output is ordinary TypeScript you can read, diff and debug;
- you inherit the JavaScript ecosystem and a mature, globally deployed platform;
- a context maps to a Worker, and an agent to a Durable Object, so the
  deployment model and the language model line up.

It has costs too, which
[Why compile to TypeScript](/book/concepts/projects-build-and-deployment/why-compile-to-typescript/)
weighs. For when to reach for Bynk at all, see
[Bynk compared to TypeScript](/book/concepts/about/bynk-compared-to-typescript/).

### And testing is built in

`suite`/`case` blocks, `expect`, generative `property`/`for all`, the
`as <tier>` dial, per-seam `stub` test doubles and
[`Val[T]`](/reference/glossary/#term-val) value fabrication ship with the
language.

## What Bynk is *not* (yet)

Bynk is pre-1.0. Some designed features — sagas and event replay — are
**deferred, not missing**, and land in later increments on the road to v1.
This book documents only what compiles today and marks planned features as
planned. See [Versioning & roadmap](/project/versioning-and-roadmap/).

## Why "Bynk"?

*Bynk* is Cornish for a **workbench** or **platform** — a solid, raised surface
you build on. It fits a language whose premise is that the structure you build
on should be part of the tool: contexts, services, and agents *are* the bench,
and your program is the work laid out on it. The double meaning is deliberate,
too — Bynk compiles to a real deployment platform.

## Next steps

- [Install Bynk](/book/introduction/install/)
- [Compile your first program](/book/tutorials/first-program/)
