---
title: Coming from TypeScript
---
If you write TypeScript, you already reach for most of Bynk's ideas — you just
assemble them by hand, by convention, and with libraries. Bynk's bet is that the
patterns you reach for *should be the language*. This page is a quick translation
table: the thing you hand-roll in TypeScript, and the Bynk construct that does it
for you.

It is an orientation, not a tutorial — for the hands-on build, start with
[Compile your first program](/book/tutorials/first-program/); for the deeper *whether and when
to choose Bynk*, see
[Bynk compared to TypeScript](/book/concepts/about/bynk-compared-to-typescript/).

## The translation table

| In TypeScript you… | In Bynk… | Note |
|---|---|---|
| hand-roll a branded type — `type OrderId = string & { readonly __brand: "OrderId" }` | declare an **opaque type** — `type OrderId = opaque String` | the brand is the language's job; see [opaque types](/reference/types/) |
| validate at the edge with `zod` or manual `if` checks | give the type a **refinement** and construct with **`.of`** (which returns a `Result`) | validation happens once, at the boundary — [refined types](/reference/refined-types/) |
| cast with `as` to assert validity | construct with **`.of`** (checked, returns a `Result`), or write a literal and let **admission** check it at compile time | there is no unchecked cast into a refined type — the predicate always runs, at run time or compile time ([admission](/book/concepts/type-system/refined-literal-admission/)) |
| return `Result`/`Either` by convention (`neverthrow`, `fp-ts`) | get **`Result[T, E]`** in the language | the caller must handle `Err` to reach `T` |
| `throw` exceptions | return **errors as values**; propagate with **`?`** | no hidden control flow — [type-system philosophy](/book/concepts/type-system/philosophy/) |
| guard `null` / `undefined` | use **`Option[T]`** (`Some` / `None`) | absence is in the type, where the compiler can see it |
| a discriminated union + `switch` with no `default` | a **sum type** read with an exhaustive **`match`** | the compiler checks every variant is handled |
| an `interface` of fields | a **record** | [type system](/reference/types/) |
| wire dependencies by hand or with decorators | declare a **capability**, ask for it with **`given`**, and supply a **provider** | dependencies are explicit and checked — [capabilities](/reference/capabilities/) |
| write a Worker `fetch` handler and a router | write an **`from http`** service and let Bynk emit the Worker | [HTTP](/reference/http/) |
| hand-write a Durable Object class | declare an **agent** — a key and `store` fields, written with **`:=`** and committed atomically when the handler returns | [the agent model](/book/concepts/agents-and-state/the-agent-model/) |

## The shift in feel

Two themes run through the table. First, **things you assert by convention become
things the compiler checks**: a brand, a validated value, an exhaustive switch, a
declared dependency. Second, **structure you express in folders and framework glue
becomes part of the language**: a
[context](/reference/glossary/#term-context) is a boundary, a
[service](/reference/glossary/#term-service) groups handlers, an
[agent](/reference/glossary/#term-agent) owns state. You write less plumbing, and the compiler can hold
you to the shape you intended.

The cost side of that trade — a new toolchain, a smaller ecosystem — is discussed
honestly in
[Bynk compared to TypeScript](/book/concepts/about/bynk-compared-to-typescript/).

## Where to next

- Build something: [Compile your first program](/book/tutorials/first-program/) is a five-minute
  warm-up; tutorials 2–6 then build one real service end to end.
- Keep the [glossary](/reference/glossary/) open for any unfamiliar term.
- Skim [the type-system philosophy](/book/concepts/type-system/philosophy/) for
  the reasoning behind the table.
