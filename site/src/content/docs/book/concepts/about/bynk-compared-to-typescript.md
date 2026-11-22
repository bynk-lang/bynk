---
title: Bynk compared to TypeScript
---
Bynk compiles *to* TypeScript, so a natural question is: why not just write
TypeScript? This page positions the two and suggests when to reach for Bynk.

## Where they overlap

Both are statically typed, both run on the JavaScript platform, and both produce
TypeScript you can deploy. If you are comfortable in TypeScript, Bynk's emitted
output will look familiar — branded types, discriminated unions, plain async
functions.

## Where Bynk goes further

- **Refinement is in the type system.** TypeScript can model a `number`, but "a
  number between 0 and 150" is a runtime check you remember to write. In Bynk it
  is a [refined type](/book/concepts/type-system/philosophy/): the invalid value cannot be
  constructed, and validation at the boundary is forced by the type of `.of`.
- **Opacity is enforced, not conventional.** TypeScript's branding tricks are
  opt-in and bypassable. Bynk's [opaque types](/book/concepts/type-system/philosophy/) are a
  language feature with enforced construction and boundaries.
- **No exceptions, no `null`.** Failure and absence are
  [values](/book/concepts/type-system/philosophy/) (`Result`, `Option`) the caller must
  handle, rather than control flow you can forget to catch.
- **Architecture is in the language.** Contexts, services, agents, capabilities,
  and the `uses`/`consumes` graph are language constructs the compiler checks —
  not patterns layered on top. See
  [How a Bynk program is shaped](/book/concepts/program-structure/how-a-program-is-shaped/).
- **State and deployment are modelled.** A stateful [agent](/book/concepts/agents-and-state/the-agent-model/)
  maps to a Durable Object; a context maps to a Worker. The language model and
  the deployment model line up.

The cost of all this is that Bynk is a smaller, younger language with a fixed set
of constructs, where TypeScript is vast and mature.

## When to reach for Bynk

Bynk is most worth it when:

- You are building **services**, especially on **Cloudflare Workers**, where the
  agent-to-Durable-Object and context-to-Worker mappings pay off.
- **Correctness matters** enough that making illegal states unrepresentable, and
  forcing validation at boundaries, is worth adopting a dedicated language.
- You want the **architecture written down and checked**, not maintained by
  convention and review.

Reach for plain TypeScript when you need the full breadth of the ecosystem and
its libraries directly, when the project is small enough that Bynk's structure is
overhead rather than help, or when the team's investment in TypeScript outweighs
the gains.

## See also

- [Why Bynk exists](/book/about/why-bynk-exists/) · [Why compile to TypeScript](/book/concepts/projects-build-and-deployment/why-compile-to-typescript/)
