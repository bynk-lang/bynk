---
title: Split into contexts
---
The shortener is one file and one **context**, and a context is what Bynk
deploys: on Cloudflare, each one becomes its own Worker. So far, the part that
answers HTTP and the part that stores links are one program, deployed together.

In this chapter you split them. The links and their storage move into a
context of their own, the HTTP routes stay in another, and the words both of
them use move into a `commons` they share. You'll end with three files and two
Workers, and with the dependency between them written down where the compiler
can check it.

## Why split

A context draws a line around a part of your system: what it owns, and what it
offers to the rest. Splitting the shortener at its natural seam gives you:

- **A small, stable core.** The part that stores links answers a couple of
  calls and knows nothing about HTTP. You can change the API's routes without
  touching it, or deploy them separately.
- **Dependencies you can read.** One context calls another only if it says
  `consumes` that context. The architecture isn't a convention: it's in the
  code, and the compiler checks it.

## The shared words, in a `commons`

Both halves need the same types: `ShortCode`, `Url`, `LinkError` and the views.
Types and pure functions that more than one context uses go in a **`commons`**.
A `commons` has no state and no effects, only values and functions, and the
compiler holds it to that.

Create `links.bynk`, and move into it the types, `describe`, `HOME`,
`checkTarget` and `newLink`:

```bynk,fragment
commons links

type ShortCode = String where MinLength(6) && MaxLength(8)

type Url = String where MinLength(1) && MaxLength(2048)

type LinkError = enum { AlreadyExists, NotFound, Invalid, SelfLink }
```

A context brings a `commons` in with `uses links`. Everything the `commons`
declares is then in scope, as if it were written there.

## The registry: storage and codes

Create `registry.bynk`. It owns everything to do with keeping links: the
`CodeGen` capability and its provider, the `Link` agent, and the `create`
service. It gains one more service, `resolve`, so that other contexts can look
a link up:

```bynk,fragment
service resolve {
	on call(code: ShortCode) -> Effect[Result[ResolveView, LinkError]] {
		Link(code).resolve()
	}
}
```

`create` now returns the whole `CreatedView`, because the HTTP side needs both
the code and the target to answer with.

## One thing a context can't do with a `commons` type

Move the agent across unchanged, and the compiler stops you:

```text
[bynk.context.rebrand_construction] Error: cannot construct `LinkError` here —
its constructors are not in scope in this context
   │
25 │       Some(_) => Err(AlreadyExists),
```

Each context gets its own copy of the types it `uses` when it is built. A
context can read a `commons` sum, matching on its variants as much as it likes,
but it builds new values of it through the `commons`. So give `links` a small
function for each variant the registry needs to make:

```bynk,fragment
fn alreadyExists() -> LinkError {
	AlreadyExists
}

fn notFound() -> LinkError {
	NotFound
}
```

The agent writes `Err(alreadyExists())` and `Err(notFound())`. Matching, as
`failure` does, needs nothing new.

## The edge: HTTP only

What's left in `shortener.bynk` is the HTTP front door. It `uses links` for the
types, and `consumes registry` to call the registry's services:

```bynk,fragment
on POST("/links") (
	body: CreateLinkRequest
) -> Effect[HttpResult[CreatedView]] by Visitor given Logger {
	let created <- registry.create(body.target)
	match created {
		Ok(view) => {
			let _ <- Logger.info("created \(view.code)")
			Created(view)
		},
		Err(error) => failure[CreatedView](error),
	}
}
```

A call to another context is an effect, like a capability call: it crosses a
boundary, so it uses `<-`. The handler no longer needs `CodeGen`. Making codes
is the registry's business now, and its own `given` says so.

`failure` stays here, because choosing an HTTP status is the edge's job. The
registry speaks `LinkError`, and only the edge turns it into a response.

## The files so far

Three files, side by side in the project directory. Each file is named after
the unit it declares.

`links.bynk`:

```bynk,project=shortener
commons links

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

fn alreadyExists() -> LinkError {
	AlreadyExists
}

fn notFound() -> LinkError {
	NotFound
}
```

`registry.bynk`:

```bynk,project=shortener
context registry

uses links
consumes bynk { Random }

capability CodeGen {
	fn next() -> Effect[String]
}

provides CodeGen = RandomCodeGen given Random {
	fn next() -> Effect[String] {
		let id <- Random.uuid()
		"\(id)".slice(0, 8)
	}
}

agent Link {
	key code: ShortCode

	store target: Cell[Option[Url]]
	store hits: Cell[Int]

	on call register(url: Url) -> Effect[Result[(), LinkError]] {
		match target {
			Some(_) => Err(alreadyExists()),
			None => {
				target := Some(url)
				Ok(())
			},
		}
	}

	on call resolve() -> Effect[Result[ResolveView, LinkError]] {
		let Some(url) = target else { Err(notFound()) }
		let next = hits + 1
		hits := next
		Ok(ResolveView { target: url, hits: next })
	}
}

service create {
	on call(target: Url) -> Effect[Result[CreatedView, LinkError]] given CodeGen {
		let raw <- CodeGen.next()
		let view = newLink(raw, target)?
		let registered <- Link(view.code).register(view.target)
		let _ = registered?
		Ok(view)
	}
}

service resolve {
	on call(code: ShortCode) -> Effect[Result[ResolveView, LinkError]] {
		Link(code).resolve()
	}
}
```

`shortener.bynk`:

```bynk,project=shortener
context shortener

uses links
consumes bynk { Logger }
consumes registry

type CreateLinkRequest = {
	target: Url,
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
	on POST("/links") (
		body: CreateLinkRequest
	) -> Effect[HttpResult[CreatedView]] by Visitor given Logger {
		let created <- registry.create(body.target)
		match created {
			Ok(view) => {
				let _ <- Logger.info("created \(view.code)")
				Created(view)
			},
			Err(error) => failure[CreatedView](error),
		}
	}

	on GET("/links/:code") (code: ShortCode) -> Effect[HttpResult[ResolveView]] by Visitor {
		let resolved <- registry.resolve(code)
		match resolved {
			Ok(view) => Ok(view),
			Err(error) => failure[ResolveView](error),
		}
	}
}
```

## Build and run

Compile for Cloudflare as before:

```sh
bynk compile . --output out --target workers
```

`out/workers/` now has two Workers, `registry/` and `shortener/`. The
`consumes registry` in `shortener.bynk` becomes a service binding between them:
the edge calls the registry directly, inside Cloudflare, without going over the
public internet.

To run both locally, `bynk dev` is all you need. It serves every Worker in the
project and wires the binding between them:

```sh
bynk dev
```

`POST /links` and `GET /links/:code` behave just as they did before the split.

## What you have done

You split the shortener at its natural seam. `commons links` holds the words
both sides share, `registry` owns the links and the codes, and `shortener` is
only the HTTP edge. `uses links` and `consumes registry` say exactly how they
depend on each other, and the compiler checks both. You met the one rule that
comes with sharing a sum type, and you built and ran two Workers.

Next, we prove the registry works, with tests.

➡️ **Next: [Test it](/book/tutorials/06-testing/)**

---

*How contexts, commons, `uses` and `consumes` fit together: see
[How a program is shaped](/book/concepts/program-structure/how-a-program-is-shaped/).
For more on calling another context, see
[Consume another context's services](/book/guides/program-structure/consume-services/),
and to share a capability rather than a service, see
[Share a capability across contexts](/book/guides/effects-and-capabilities/share-across-contexts/).
The rule about building commons types is
[`bynk.context.rebrand_construction`](/reference/diagnostics/bynk.context.rebrand_construction/),
and running several Workers locally is in
[Run your project locally](/book/guides/projects-build-and-deployment/run-locally/#multi-context-projects).*
