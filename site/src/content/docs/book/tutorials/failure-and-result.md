---
title: Handle failure with Result
---
The shortener can already say no. A malformed request body is turned away with
a `400` before your handler runs, and an out-of-range short code can't be built
at all. Those are rules about **shape**, and the types enforce them. A service
also has rules about **meaning**: things that are well-formed but still wrong.
This chapter gives the shortener one such rule, and shows how Bynk handles the
failures it produces, from the function that finds them to the HTTP response
that reports them.

Keep editing `shortener.bynk`.

## Errors are values

Bynk has no exceptions and no `null`. A step that can fail says so in its type:
it returns a `Result[T, E]`, which is either `Ok` with a `T` or `Err` with an
`E`. You met one in [Make illegal states unrepresentable](/book/tutorials/04-refined-types/):
`ShortCode.of(raw)` returns a `Result`, because the string might not fit.

Because failure is a value, nothing can skip it. A caller can't use the `T`
inside a `Result` without deciding what to do about the `E`, and the compiler
checks that it did. The rest of this chapter is about deciding well.

## A rule the types can't express

A short link must not point back at the shortener itself: following it would
only lead to another short link, and a chain of them could loop. Whether a
`Url` starts with the shortener's own address is not something a refinement
can say, so it is a check in code, and its failure is a value.

First, name the failure. `LinkError` from
[Model your data with types](/book/tutorials/03-modelling-data/) gains a
variant, and `describe` an arm for it:

```bynk,fragment
type LinkError = enum { AlreadyExists, NotFound, Invalid, SelfLink }

fn describe(error: LinkError) -> String {
	match error {
		AlreadyExists => "code already in use",
		NotFound => "no such code",
		Invalid => "invalid code",
		SelfLink => "a link can't point back at the shortener",
	}
}
```

The compiler holds you to that second change: a `match` must cover every
variant, so adding `SelfLink` without a new arm is a compile error.

Then the check. A named constant holds the shortener's address, and
`checkTarget` returns the URL unchanged, or `Err(SelfLink)`:

```bynk,fragment
const HOME: String = "https://sho.rt/"

fn checkTarget(url: Url) -> Result[Url, LinkError] {
	if url.startsWith(HOME) { Err(SelfLink) } else { Ok(url) }
}
```

`url.startsWith` is the `String` method a `Url` keeps from its base type, as you
saw with `ShortCode` in the last chapter.

## Compose the steps with `?`

Making a link now has two steps that can fail: checking the target, and
validating the code. `newLink` runs them in order and stops at the first
failure:

```bynk,fragment
fn newLink(raw: String, target: Url) -> Result[CreatedView, LinkError] {
	let checked = checkTarget(target)?
	let code = ShortCode.of(raw).mapErr((_) => Invalid)?
	Ok(CreatedView { code: code, target: checked })
}
```

Two tools do the work:

- **`?`** unwraps an `Ok`, or returns the `Err` from the function at once. The
  rest of the function only ever sees the good value, so there is no `match`
  for each step.
- **`.mapErr`** changes the error inside a `Result`. `ShortCode.of` fails with a
  `ValidationError`, but `newLink` speaks `LinkError`. `.mapErr((_) => Invalid)`
  turns one into the other, so `?` can pass it on. `?` only passes on an error
  the function's own return type can hold, and the compiler checks that.

## One place an error becomes a response

A `LinkError` is about links. The client needs an HTTP status. Choosing the
status for each error is a decision, so make it once, in one function:

```bynk,fragment
fn failure[T](error: LinkError) -> HttpResult[T] {
	match error {
		SelfLink => BadRequest(describe(error)),
		AlreadyExists => Conflict(describe(error)),
		NotFound => NotFound,
		Invalid => ServerError(describe(error)),
	}
}
```

Each answer is a choice about whose fault it was. A target that points back at
the shortener is the client's mistake (`400`). A code that is already taken is
a conflict with the current state (`409`). A generated code that fails its own
validation is ours (`500`).

`failure` works for any response type, so it takes the type as a parameter, `T`.
Nothing in its argument says which `T` a handler wants, so a handler names it
when it calls: `failure[CreatedView](error)`.

Bynk doesn't turn errors into responses for you, by design. A status is part of
what your API promises its clients, so the choice stays in your code, where you
can read it.

## Use it in the handler

`POST /links` now makes the link and answers in two lines:

```bynk,fragment
on POST("/links") (body: CreateLinkRequest) -> Effect[HttpResult[CreatedView]] by Visitor {
	match newLink("abc123", body.target) {
		Ok(view) => Created(view),
		Err(error) => failure[CreatedView](error),
	}
}
```

A body whose `target` is `https://sho.rt/abc123` is now answered with `400` and
`{"error":"a link can't point back at the shortener"}`.

## Three more tools

You'll use these in the next chapter, when the shortener can look things up:

- **`?` on an `Option` answers `404`.** Inside a handler that returns an
  `HttpResult`, `found?` gives the value if there is one, and otherwise ends the
  handler with `NotFound`. That is right for "the thing you asked for isn't
  there", and only there.
- **`let … else` names another answer.** When a missing value means something
  other than `404`, say what:

  ```bynk,fragment
  let Some(url) = target else { Err(NotFound) }
  ```

  The `else` block runs when the pattern doesn't match, and must leave the
  function. With `else match { … }`, it can work out its answer from what it
  got instead, such as the error inside an `Err`.
- **`unreachable` marks a case that can't happen.** When a `match` has an arm
  that your program rules out, write `unreachable("why")` rather than inventing
  an answer. If you're wrong, the program faults loudly instead of returning
  something plausible. See
  [Fault on the impossible](/book/guides/type-system/fault-on-the-impossible/).

## The file so far

```bynk
context shortener

type ShortCode = String where MinLength(6) && MaxLength(8)

type Url = String where MinLength(1) && MaxLength(2048)

type LinkError = enum { AlreadyExists, NotFound, Invalid, SelfLink }

fn describe(error: LinkError) -> String {
	match error {
		AlreadyExists => "code already in use",
		NotFound => "no such code",
		Invalid => "invalid code",
		SelfLink => "a link can't point back at the shortener",
	}
}

const HOME: String = "https://sho.rt/"

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

fn checkTarget(url: Url) -> Result[Url, LinkError] {
	if url.startsWith(HOME) { Err(SelfLink) } else { Ok(url) }
}

fn newLink(raw: String, target: Url) -> Result[CreatedView, LinkError] {
	let checked = checkTarget(target)?
	let code = ShortCode.of(raw).mapErr((_) => Invalid)?
	Ok(CreatedView { code: code, target: checked })
}

fn failure[T](error: LinkError) -> HttpResult[T] {
	match error {
		SelfLink => BadRequest(describe(error)),
		AlreadyExists => Conflict(describe(error)),
		NotFound => NotFound,
		Invalid => ServerError(describe(error)),
	}
}

service api from http {
	on POST("/links") (body: CreateLinkRequest) -> Effect[HttpResult[CreatedView]] by Visitor {
		match newLink("abc123", body.target) {
			Ok(view) => Created(view),
			Err(error) => failure[CreatedView](error),
		}
	}

	on GET("/links/:code") (code: ShortCode) -> Effect[HttpResult[ResolveView]] by Visitor {
		NotFound
	}
}
```

```sh
bynk compile . --output out --target workers
```

## What you have done

You gave the shortener a rule about meaning, and handled its failure the Bynk
way: as a value. `checkTarget` returns a `Result`, `newLink` composes two
fallible steps with `?` and `.mapErr`, and one function, `failure`, decides the
HTTP status for every `LinkError`. The handler is two lines, and every error it
can meet has an answer the compiler has checked.

The API still mints the same code and forgets it. Next, we give the shortener a
memory, and the tools from this chapter keep its handlers short.

➡️ **Next: [Add a stateful agent](/book/tutorials/05-stateful-agent/)**

---

*Why errors are values, and what Bynk gives up by having no exceptions: see
[Errors as values](/book/concepts/type-system/philosophy/#errors-as-values-no-hidden-control-flow).
For more ways to work with them, see
[Work with `Result` and optional values](/book/guides/type-system/result-and-optionals/).
Every method is in the [types reference](/reference/types/#option-and-result-methods),
and the HTTP side, `?` on an `Option` and `let … else`, is in the
[HTTP reference](/reference/http/). To try `Result` and `Option` in the browser,
see [Result and Option](/by-example/snippets/result-and-option/).*
