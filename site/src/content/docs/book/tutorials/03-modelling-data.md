---
title: Model your data with types
---
In [Tutorial 2](/book/tutorials/02-http-service/) the shortener echoed strings around. Now we
give it a real data model. Bynk gives you three ways to shape data, and choosing
the right one is most of the work of modelling a domain well:

- **Records** group related fields together.
- **Sum types** (and their no-payload form, **enums**) express "one of several
  alternatives".
- **Opaque types** give a value a distinct identity so it cannot be confused
  with another value of the same underlying shape.

We will model the shortener's requests, responses, and errors, and meet `match`.
Keep editing `shortener.bynk`; we compile it at the end.

## Records group fields

A **[record](/book/reference/glossary/#term-record)** groups fields into a single
value. The shortener needs a request
body and two response shapes:

```bynk
type CreateLinkRequest = {
  target: String,
}

type CreatedView = {
  code: String,
  target: String,
}

type ResolveView = {
  target: String,
  hits: Int,
}
```

(The `String` fields become precise refined types in
[Tutorial 4](/book/tutorials/04-refined-types/); for now plain strings keep us moving.) You
construct a record by naming it and giving every field a value:

```bynk,fragment
Created(CreatedView { code: "abc123", target: body.target })
```

Records are immutable. To produce a changed copy, use the **spread** form
`{ ...r, … }`, which copies every field and overrides the ones you name (we will
use it for agent state in [Tutorial 5](/book/tutorials/05-stateful-agent/)). A record compiles
to a TypeScript `interface` with `readonly` fields:

```typescript
export interface ResolveView {
  readonly target: string;
  readonly hits: number;
}
```

## A sum type for errors

Creating or resolving a link can go wrong in a few distinct ways. That is exactly
what a **[sum type](/book/reference/glossary/#term-sum-type)** expresses — a value
that is one of several named variants.
When none of the variants carries a payload, the shorthand is an **enum**:

```bynk
type LinkError = enum {
  AlreadyExists,
  NotFound,
  Invalid,
}
```

`LinkError` is one of three variants. You construct one by naming it —
`AlreadyExists`. It compiles to a discriminated union plus a constructor
namespace:

```typescript
export type LinkError =
    { readonly tag: "AlreadyExists" }
  | { readonly tag: "NotFound" }
  | { readonly tag: "Invalid" };

export const LinkError = {
  AlreadyExists: { tag: "AlreadyExists" } as LinkError,
  NotFound: { tag: "NotFound" } as LinkError,
  Invalid: { tag: "Invalid" } as LinkError,
};
```

(A variant can also carry data — `Shipped(tracking: String)` — but our errors are
plain tags, so an `enum` is the right tool. See the
[type reference](/book/reference/types/) for payload-carrying variants.)

## Read a sum type with `match`

To read a sum type, you `match` on it. `match` forces you to handle **every**
variant, so adding a case later makes the compiler revisit every place that
inspects the type:

```bynk,fragment
fn describe(error: LinkError) -> String {
  match error {
    AlreadyExists => "code already in use"
    NotFound => "no such code"
    Invalid => "invalid code"
  }
}
```

If you forget a variant, the program does not compile — there is no way to fall
through a case by accident. `match` compiles to a `switch` on the tag:

```typescript
export function describe(error: LinkError): string {
  switch (error.tag) {
    case "AlreadyExists": {
      return "code already in use";
    }
    case "NotFound": {
      return "no such code";
    }
    case "Invalid": {
      return "invalid code";
    }
  }
  throw new globalThis.Error("non-exhaustive match");
}
```

## Opaque types, in one minute

The third tool is the **opaque type**: a value backed by some base type but with
its own identity, so the compiler refuses to mix it up with another value of the
same underlying shape.

```bynk
type LinkId = opaque String   // a String, but not interchangeable with one
```

`LinkId` compiles to a *branded* type — `string & { readonly __brand: "LinkId" }`
— so a plain `String` cannot stand in for it. Opacity is the right tool when you
want identity. For the shortener's *short codes*, though, we want more than
identity — we want to guarantee the string is actually a valid code. That is a
job for **refined types**, and it is exactly where we go next.

## Compile what we have

Here is `shortener.bynk` so far — the data model wired into the API:

```bynk
context shortener

type LinkError = enum {
  AlreadyExists,
  NotFound,
  Invalid,
}

type CreateLinkRequest = {
  target: String,
}

type CreatedView = {
  code: String,
  target: String,
}

type ResolveView = {
  target: String,
  hits: Int,
}

fn describe(error: LinkError) -> String {
  match error {
    AlreadyExists => "code already in use"
    NotFound => "no such code"
    Invalid => "invalid code"
  }
}

service api from http {
  on POST("/links") (body: CreateLinkRequest) -> Effect[HttpResult[CreatedView]] by Visitor {
    Created(CreatedView { code: "abc123", target: body.target })
  }

  on GET("/links/:code") (code: String) -> Effect[HttpResult[ResolveView]] by Visitor {
    NotFound
  }
}
```

```sh
bynkc compile . --output out --target workers
```

## What you have done

You modelled the shortener's data with the core type kinds — `CreateLinkRequest`,
`CreatedView`, and `ResolveView` records, and a `LinkError` enum — and consumed
the sum with an exhaustive `match`. This is the everyday vocabulary of Bynk data
modelling.

Right now a `code` is any old `String`. Next we sharpen that: how do we stop an
*invalid* short code — too short, wrong shape — from being constructed at all?

➡️ **[Tutorial 4: Make illegal states unrepresentable](/book/tutorials/04-refined-types/)**

---

*For the reasoning behind opacity, sums, and immutable records, see
[The type-system philosophy](/book/guides/type-system/philosophy/). For
exact rules, see the [type system reference](/book/reference/types/).*
