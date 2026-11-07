---
title: What is Bynk?
---
Bynk is a statically typed programming language for building services. It is
**architecture-first**: the shape of a program — its
[contexts](/book/reference/glossary/#term-context),
[services](/book/reference/glossary/#term-service),
[agents](/book/reference/glossary/#term-agent), and the types that flow between
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

## What makes Bynk distinct

- **Make illegal states unrepresentable.** Bynk leans on
  *[refined types](/book/reference/glossary/#term-refined-type)*
  (types carrying a predicate), *[opaque types](/book/reference/glossary/#term-opaque-type)*,
  and *errors-as-values* ([`Result`](/book/reference/glossary/#term-result-option),
  `Ok`/`Some`/`None`) so that whole classes of bug cannot be
  expressed. See [The type-system philosophy](/book/guides/type-system/philosophy/).
- **Architecture in the language.** Contexts, services, and stateful *agents*
  are first-class. See [How a Bynk program is shaped](/book/guides/program-structure/how-a-program-is-shaped/).
- **Compiles to TypeScript.** You get JavaScript-ecosystem interop and a
  natural fit for Cloudflare Workers, with a static type system in front of it.
  See [Why compile to TypeScript](/book/guides/projects-build-and-deployment/why-compile-to-typescript/).
- **Testing is built in.** `suite`/`case` blocks, `expect`, generative
  `property`/`for all`, the `as <tier>` dial, per-seam `stub` test doubles, and
  [`Val[T]`](/book/reference/glossary/#term-val) value fabrication ship with the
  language.

## What Bynk is *not* (yet)

Bynk is pre-1.0. Some designed features — sagas and event replay — are
**deferred, not missing**, and land in later increments on the road to v1.
This book documents only what compiles today and marks planned features as
planned. See [Versioning & roadmap](/book/about/versioning-and-roadmap/).

## Why "Bynk"?

*Bynk* is Cornish for a **workbench** or **platform** — a solid, raised surface
you build on. It fits a language whose premise is that the structure you build
on should be part of the tool: contexts, services, and agents *are* the bench,
and your program is the work laid out on it. The double meaning is deliberate,
too — Bynk compiles to a real deployment platform.

## Next steps

- [Install Bynk](/book/introduction/install/)
- [Compile your first program](/book/tutorials/01-first-program/)
