---
title: Reach out with capabilities
---
Everything the shortener has done so far is computation: values in, values
out. Real services also reach outside themselves. They read the clock, make
random numbers, call other systems, write logs. In Bynk, every such reach goes
through a **capability**, and a handler says up front which ones it uses.

This chapter fixes something you may have noticed: every link the shortener
makes gets the same code, `abc123`. By the end, codes are random, and the
handler that makes them says exactly what it needs to do so.

Keep editing `shortener.bynk`.

## Why effects are declared

A Bynk function can't read the time, draw a random number or write a log line
unless its signature says it may. Code that only computes stays plain and easy
to test, and code that reaches outside says so where you can see it. The reach
itself is an **effect**: a value of type `Effect[T]` that, when run, does
something and produces a `T`.

## Use one of Bynk's own

Bynk ships portable capabilities for the everyday cases, including `Random` and
`Logger`. A context brings them in with `consumes`, near the top of the file:

```bynk,fragment
consumes bynk { Random, Logger }
```

A handler that uses one lists it in its `given` clause and calls it by name.
Here is a first, direct attempt at a random code:

```bynk,fragment
on POST("/links") (body: CreateLinkRequest) -> Effect[HttpResult[CreatedView]] by Visitor given Random {
	let id <- Random.uuid()
	match newLink("\(id)".slice(0, 8), body.target) {
		Ok(view) => Created(view),
		Err(error) => failure[CreatedView](error),
	}
}
```

Two things are new:

- **`let id <- Random.uuid()`** runs an effect and binds its result. Compare
  `let checked = checkTarget(target)?` from the last chapter: `=` binds a value
  that is already there, and `<-` runs an effect to get one. `Random.uuid()` is
  an `Effect[Uuid]`, so it needs `<-`.
- **`given Random`** lists the capability. Leave it out, and the compiler stops
  you at the call:

  ```text
  [bynk.given.undeclared_capability] Error: capability `Random` is used but not
  listed in the handler's `given` clause
  ```

A UUID is 32 hexadecimal digits with hyphens. The first eight are a valid
`ShortCode`, and `newLink` still checks that they are.

## A capability of your own

That works, but the handler now knows how codes are made. It shouldn't care: it
needs *a code*, and how one is made is a separate decision that you might
change, or want to replace in a test. So name the need by what it means to the
shortener:

```bynk,fragment
capability CodeGen {
	fn next() -> Effect[String]
}
```

A capability is only signatures, no bodies. A **provider** supplies the bodies.
This one makes codes from `Random`, so it says `given Random` itself:

```bynk,fragment
provides CodeGen = RandomCodeGen given Random {
	fn next() -> Effect[String] {
		let id <- Random.uuid()
		"\(id)".slice(0, 8)
	}
}
```

Now the handler asks for `CodeGen` and never sees `Random`. Which provider
stands behind `CodeGen` is decided once, for the whole context.

Eight hexadecimal digits allow about four billion codes, so two links will
rarely get the same one. When they do, the next chapter's storage answers
`409 Conflict`, through the `failure` function you already have. A busy
service would try again with a fresh code; the shortener doesn't need to yet.

## Log what happens

`Logger` is the same mechanism for a different job. With `given CodeGen,
Logger`, the handler can record each link it creates:

```bynk,fragment
on POST("/links") (
	body: CreateLinkRequest
) -> Effect[HttpResult[CreatedView]] by Visitor given CodeGen, Logger {
	let raw <- CodeGen.next()
	match newLink(raw, body.target) {
		Ok(view) => {
			let _ <- Logger.info("created \(view.code)")
			Created(view)
		},
		Err(error) => failure[CreatedView](error),
	}
}
```

`Logger.info` returns `Effect[()]`: it is run for what it does, not for a
value, so its result is bound to `_`.

## What `given` buys you

Every effect a handler can have is now in its signature. `given CodeGen,
Logger` says this handler can make codes and write logs, and nothing else: it
can't read the clock or call the network without the signature changing. The
compiler holds the handler to the list in both directions. A call to a
capability that isn't listed is an error, and a listed one the handler never
uses draws a warning.

A capability is also a seam. A test can put a stand-in where `CodeGen` goes, so
that a case gets a code it can predict. You'll do exactly that in
[Test it](/book/tutorials/06-testing/).

## The file so far

```bynk
context shortener

consumes bynk { Random, Logger }

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

capability CodeGen {
	fn next() -> Effect[String]
}

provides CodeGen = RandomCodeGen given Random {
	fn next() -> Effect[String] {
		let id <- Random.uuid()
		"\(id)".slice(0, 8)
	}
}

service api from http {
	on POST("/links") (
		body: CreateLinkRequest
	) -> Effect[HttpResult[CreatedView]] by Visitor given CodeGen, Logger {
		let raw <- CodeGen.next()
		match newLink(raw, body.target) {
			Ok(view) => {
				let _ <- Logger.info("created \(view.code)")
				Created(view)
			},
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

You let the shortener reach outside itself, and say so. `consumes bynk {
Random, Logger }` brings in two of Bynk's own capabilities. `CodeGen`, a
capability of your own, names what the handler needs, and `RandomCodeGen`
provides it from `Random`. The handler's `given CodeGen, Logger` lists every
effect it can have, and the compiler holds it to that list.

The shortener now makes a fresh code for every link, and then forgets it. Next,
we give it a memory.

➡️ **Next: [Add a stateful agent](/book/tutorials/05-stateful-agent/)**

---

*Why effects are gated, and how providers and adapters fit together: see
[Understand the capability model](/book/concepts/effects-and-capabilities/understand-the-capability-model/).
To build a provider out of others, see
[Compose a provider](/book/guides/effects-and-capabilities/compose-a-provider/).
The exact rules are in [Capabilities & providers](/reference/capabilities/),
and every capability Bynk ships is in [First-party capabilities](/reference/bynk-capabilities/).
The [link shortener project](/by-example/projects/link-shortener/) makes its
codes the same way.*
