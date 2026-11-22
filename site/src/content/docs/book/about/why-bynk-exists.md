---
title: Why Bynk exists
---
This page is about motivation: the problems Bynk is trying to solve, and the
bets it makes in solving them. It argues a position rather than documenting exact
behaviour — for that, see the [reference](/reference/).

## The problem: services drift from their design

Most services begin with a clear architecture in someone's head — these are the
boundaries, this owns that state, this calls that. Almost none of that survives
contact with the code. The architecture lives in diagrams and tribal knowledge;
the code is a pile of functions and framework glue that *implements* the design
without ever *stating* it. Over time the two drift apart, and the gap is where
bugs and onboarding pain live.

Bynk's first bet is that **the architecture should be in the language**. A
[context](/reference/glossary/#term-context) is a deployable boundary. A
[service](/reference/glossary/#term-service) groups handlers. An
[agent](/reference/glossary/#term-agent) is a named, keyed owner of state. These are not naming conventions or folder layouts — they
are constructs the compiler understands and enforces. The shape of the system is
written down, checked, and compiled, so it cannot quietly rot.

## The bet: make illegal states unrepresentable

The second bet is that a great many runtime errors are really *type* errors that
the type system was too weak to catch. An order id is "just a string", so it gets
swapped with a customer id. A percentage is "just a number", so one day it is
`240`. An optional value is "usually there", until it isn't.

Bynk pushes hard on the type system to close these gaps:

- **[Refined types](/reference/glossary/#term-refined-type)** carry a
  predicate, so `Age = Int where InRange(0, 150)` is a distinct type whose invalid
  values *cannot be constructed*. An out-of-range literal is a compile error, not
  a runtime surprise.
- **[Opaque types](/reference/glossary/#term-opaque-type)** give a value a
  nominal identity, so an `OrderId` and a `CustomerId` never mix even though both
  are strings underneath.
- **Errors are values.** Operations that can fail return a
  [`Result`](/reference/glossary/#term-result-option); absence is an `Option`.
  There are no exceptions to forget to catch — the type forces the caller to
  handle both outcomes.
- **Agent state must be [zeroable](/reference/glossary/#term-zeroable)**, so a
  never-seen key has a well-defined
  starting state, and "uninitialised" is expressed honestly with `Option`.

The aim throughout is to move whole categories of bug from *runtime* to *compile
time* — to make the wrong program fail to build. Take the percentage that is "just
a number". In TypeScript, the impossible value compiles:

```typescript
const age: number = 240; // compiles — 240 is a perfectly good `number`
```

In Bynk, it does not:

```bynk,fail
{{#include ../../diagnostics/refine_out_of_range.bynk}}
```

```text
{{#include ../../diagnostics/refine_out_of_range.txt}}
```

The same move plays out for the id-swap and the unhandled `Result` (see
[the type-system philosophy](/book/concepts/type-system/philosophy/)) and for unzeroable agent
state (see [the agent model](/book/concepts/agents-and-state/the-agent-model/)).

## The pragmatic choice: compile to typed TypeScript

The third bet is about reach. A new language with a new runtime asks the world to
adopt an entire ecosystem before it can be useful. Bynk instead **compiles to
typed TypeScript** and targets **Cloudflare Workers**. That choice buys a lot:

- The output is ordinary TypeScript you can read, diff, and debug. There is no
  opaque bytecode and no mystery runtime.
- You inherit the JavaScript ecosystem and a mature, globally deployed serverless
  platform instead of reinventing them.
- A context maps cleanly to a Worker, and a stateful agent to a Durable Object —
  the deployment model and the language model line up.

It also has costs, which the companion page
[Why compile to TypeScript](/book/concepts/projects-build-and-deployment/why-compile-to-typescript/) discusses honestly.

## What this adds up to

Put together, Bynk is an attempt to make the *correct* way to build a service
also the *easy* way: state the architecture so it stays true, lean on the type
system so illegal states never compile, and ride a proven runtime instead of a
bespoke one. Whether that trade is worth it for you depends on the work — see
[Bynk compared to TypeScript](/book/concepts/about/bynk-compared-to-typescript/) for when to reach
for it.

Underneath the three bets is a quieter principle: the constraints are
**pedagogical by design**. Bynk makes the right shape the path of least
resistance — the expressible program tends to be the correct one — and when you
do reach for a shape it will not allow, it does not merely fail. It declines the
program and names the invariant you broke, as the refusals above (and on the
[type-system](/book/concepts/type-system/philosophy/) and [agent](/book/concepts/agents-and-state/the-agent-model/) pages)
show: the diagnostic is the lesson, delivered at the moment it is most useful.
Bynk is a service-tier language, not a teaching tool — but a language that
explains every refusal is one you come to understand *as you use it*.

If you have not yet, the fastest way to get the feel of these ideas is to build
something: start with [Tutorial 1](/book/tutorials/01-first-program/).
