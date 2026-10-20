---
title: Make illegal states unrepresentable
---
The types from [Tutorial 3](/book/tutorials/03-modelling-data/) describe the *shape* of the
shortener's data. **Refined types** go further: they describe which *values* are
allowed. A short code is not just any string — it has a length and a permitted
character set. A target is not just any string — it must look like a URL. With
refined types we push those rules into the type itself, so an invalid value
cannot be constructed.

Keep editing `shortener.bynk`.

## Declare a refined type

A [refined type](/book/reference/glossary/#term-refined-type) is a base type plus a
predicate, written with `where`. Give the
shortener real `ShortCode` and `Url` types:

```bynk
type ShortCode = String where MinLength(6) && MaxLength(8)
type Url = String where MinLength(1) && MaxLength(2048)
```

`ShortCode` is a `String`, but only one of length 6–8; you combine predicates
with `&&`. Bynk ships a fixed set — numeric ones like `NonNegative`, `Positive`,
and `InRange(lo, hi)`; string ones like `NonEmpty`, `MinLength(n)`, `MaxLength(n)`,
`Length(n)`, and `Matches(regex)`. For a code we really want a character set too,
which `Matches` gives us — `String where Matches("[a-zA-Z0-9]{6,8}")` — but the
length bounds are enough to see the idea. The
[refined-type reference](/book/reference/refined-types/) lists every predicate.

Now swap the plain `String` fields in the data model for these types:

```bynk,fragment
type CreateLinkRequest = { target: Url }
type CreatedView       = { code: ShortCode, target: Url }
```

## Admit a literal — checked at compile time

When you write a literal where a refined type is expected, Bynk checks it **at
compile time** and admits it directly. No validation call, no error handling:

```bynk,fragment
fn exampleCode() -> ShortCode {
  "abc123"
}
```

`"abc123"` is a valid `ShortCode`, so this compiles, lowering to an inline brand
cast `("abc123" as ShortCode)` — the check happened in the compiler, so none is
needed at runtime:

```typescript
export function exampleCode(): ShortCode {
  return ("abc123" as ShortCode);
}
```

Now try a value that is *not* valid — say `"xy"`, which is too short. The
compiler refuses it:

```text
[bynk.refine.literal_violates] Error: literal "xy" does not satisfy `MinLength` required by type `ShortCode`
```

This is the heart of "make illegal states unrepresentable": a nonsensical
`ShortCode` is not a runtime bug to be caught later — it is a program that does
not compile.

## Validate untrusted input with `.of`

Compile-time admission only works for literals you write yourself. Real input —
an HTTP path segment, a request body, a generated code — is not known at compile
time, so it must be *checked at runtime*. Every refined type has an `.of`
constructor for exactly this, and it **always** returns a `Result`:

```bynk,fragment
ShortCode.of(raw)   // Result[ShortCode, ValidationError]
```

`ShortCode.of(raw)` returns `Ok(code)` if `raw` is a valid code, or
`Err(validationError)` if not. The generated `.of` carries the predicate as a
runtime check:

```typescript
of(value: string): Result<ShortCode, ValidationError> {
  if (!(value.length >= 6)) {
    return Err({ field: "ShortCode", message: "length must be at least 6", value });
  }
  if (!(value.length <= 8)) {
    return Err({ field: "ShortCode", message: "length must be at most 8", value });
  }
  return Ok(value as ShortCode);
}
```

This is what makes the **400-at-the-boundary** behaviour from Tutorial 2 precise:
because `CreateLinkRequest.target` is now a `Url`, the body deserialiser validates
the URL shape and rejects a malformed one with `400` before the handler runs.

## Handle the `Result`

Because `.of` returns a `Result`, the caller must deal with both outcomes. You
have two common ways.

**Propagate with `?`.** Inside a function that itself returns a `Result`, the `?`
operator unwraps an `Ok` or returns early on an `Err`:

```bynk,fragment
fn parseCode(raw: String) -> Result[ShortCode, ValidationError] {
  let code = ShortCode.of(raw)?
  Ok(code)
}
```

**Branch with `match`.** When you want to handle each case explicitly, match on
the `Result` — which is exactly what the shortener's handlers do with a
generated code:

```bynk,fragment
match ShortCode.of(raw) {
  Ok(code) => Created(CreatedView { code: code, target: body.target })
  Err(_)   => ServerError("generated an invalid code")
}
```

## A note on `.unsafe`

A refined type has **no** `.unsafe` — you cannot bypass its predicate. A value
enters either through `.of` (validated at run time, returns a `Result`) or through
compile-time literal admission (checked by the compiler). That is the whole point:
every `ShortCode` in a checked program has provably satisfied its refinement. The
unchecked `.unsafe` constructor exists only for **opaque** types, and only within
the opaque type's defining commons.

## Call a base method on a refined value

A refined type keeps the read-only methods of its base — you do **not** have to
unwrap it first. Calling one returns the **base** type (a read that leaves the
refinement behind), which is exactly what you want when you are formatting or
comparing:

```bynk,fragment
fn normalise(code: ShortCode) -> String {
  code.toUpper()          // ShortCode's String kernel; result : String
}
```

`code.length()`, `code.slice(0, 3)`, and the rest of the `String` kernel work the
same way. If you declare your own method on the refined type, yours wins; the
inherited kernel is only the fallback. See
[Inherited base methods](/book/reference/refined-types/#inherited-base-methods)
for the full rule.

## The file so far

```bynk
context shortener

type ShortCode = String where MinLength(6) && MaxLength(8)
type Url = String where MinLength(1) && MaxLength(2048)

type LinkError = enum {
  AlreadyExists,
  NotFound,
  Invalid,
}

fn describe(error: LinkError) -> String {
  match error {
    AlreadyExists => "code already in use"
    NotFound => "no such code"
    Invalid => "invalid code"
  }
}

type CreateLinkRequest = {
  target: Url,
}

type CreatedView = {
  code: ShortCode,
  target: Url,
}

type ResolveView = {
  target: Url,
  hits: Int,
}

service api from http {
  on POST("/links") (body: CreateLinkRequest) -> Effect[HttpResult[CreatedView]] by Visitor {
    match ShortCode.of("abc123") {
      Ok(code) => Created(CreatedView { code: code, target: body.target })
      Err(_) => ServerError("invalid code")
    }
  }

  on GET("/links/:code") (code: ShortCode) -> Effect[HttpResult[ResolveView]] by Visitor {
    NotFound
  }
}
```

```sh
bynkc compile . --output out --target workers
```

## What you have done

You constrained the shortener's values with refined types, watched the compiler
reject an invalid literal *before runtime*, validated untrusted input with `.of`,
and handled the resulting `Result` with both `?` and `match`. An invalid short
code now has nowhere to live.

The API still mints a placeholder code and forgets it. Next we give the shortener
a memory — somewhere to actually store a link.

➡️ **[Tutorial 5: Add a stateful agent](/book/tutorials/05-stateful-agent/)**

---

*Why does admission work this way, rather than overloading `.of`? See
[The refined-literal admission model](/book/guides/type-system/refined-literal-admission/).
For the philosophy, see [The type-system philosophy](/book/guides/type-system/philosophy/).*
