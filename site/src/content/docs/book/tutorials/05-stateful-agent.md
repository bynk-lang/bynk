---
title: Add a stateful agent
---
Everything so far has been stateless: a request comes in, a value goes out,
nothing is remembered. A URL shortener has to remember — which code maps to which
URL, and how often each was followed. In Bynk, the unit of state is an
**[agent](/reference/glossary/#term-agent)**:
a named thing, identified by a key, that owns some state and exposes handlers to
read and change it.

We give the shortener a `Link` agent. Keep editing `shortener.bynk`.

## Declare an agent

```bynk,fragment
agent Link {
  key code: ShortCode

  store target: Cell[Option[Url]]
  store hits:   Cell[Int]

  on call register(url: Url) -> Effect[Result[(), LinkError]] {
    match target {
      Some(_) => Err(AlreadyExists)
      None => {
        target := Some(url)
        Ok(())
      }
    }
  }
}
```

Three parts make up the agent:

- `key code: ShortCode` — the identity. Each distinct `ShortCode` is a separate
  link with its own state. The refined `ShortCode` from
  [Make illegal states unrepresentable](/book/tutorials/04-refined-types/) means a link can only ever be keyed by a
  valid code.
- `store target: Cell[Option[Url]]` and `store hits: Cell[Int]` — the data this
  agent owns, one `store` field per value: the target URL (once registered) and a
  hit counter. A `Cell[T]` is a single stored value, read by its bare name and
  written with `:=`.
- `on call register(...)` — a handler. Handlers return an `Effect[T]` because
  they touch state. `register` stores the target the first time, and reports
  `AlreadyExists` if the code is taken.

## Store fields need a starting value

Here is the rule that shapes agent state: **every `store` field must have a
starting value**. When you address a link whose code has never been seen, Bynk
initialises its state automatically — there is no constructor to call first — so
each field needs a well-defined start. A field gets one in one of two ways: its
type has a natural **zero** (`Int` → `0`, `Bool` → `false`, `String` → `""`,
`Option[T]` → `None`), or you give it an explicit **initialiser** with `=`.

Both our fields are fine on the zero: a brand-new link starts with `target: None`
(no URL yet) and `hits: 0`. That `None` is doing real work — it *means* "this code
has never been registered", which is exactly what `resolve` will check.

A field whose type *excludes* its natural zero, and which you give no initialiser,
is rejected. You might reach for a positive count on the hits
(`type Hits = Int where Positive`) — but a fresh link has had `0` hits, and
`Positive` excludes `0`:

```bynk,fragment
store hits: Cell[Hits]   // no zero, and no initialiser
```

```text
[bynk.agents.non_zeroable_state_field] agent `Link` store field `hits` has no
defined zero value, so a fresh key cannot be initialised
```

Give it an explicit start (`store hits: Cell[Hits] = 1`), or — when
you genuinely need "not set yet" — reach for `Option`, as we did for `target`.

## Read and update state

Inside a handler, **read** a `store` field by its bare name. To **change** it,
assign with `:=`. When the new value is computed from the old one, read the old
value into a local first (a `:=` whose right-hand side names its own field is
rejected, to keep read-modify-write visible). Add a `resolve` handler that returns
the target and counts the hit:

```bynk,fragment
  on call resolve() -> Effect[Result[ResolveView, LinkError]] {
    let Some(url) = target else { Err(NotFound) }
    let next = hits + 1
    hits := next
    Ok(ResolveView { target: url, hits: next })
  }
```

The `let … else` from [Handle failure with `Result`](/book/tutorials/failure-and-result/)
does the work here: a link that was never registered has no target, so
`resolve` answers `Err(NotFound)` and reads no further.

There is no `commit` step: every `store` write a handler makes is collected and
**committed atomically when the handler returns**. If the handler faults partway
through, nothing is persisted — the writes never reach storage.

## See what it compiles to

The agent becomes a class that loads its state on entry and persists once at the
end. The zero value is baked in as `__zeroOfLinkState`:

```typescript
const __LinkRegistry = new __StateRegistry();
function __zeroOfLinkState(): LinkState { return { target: None, hits: 0 }; }

export class Link {
  // ...
  private async loadState(): globalThis.Promise<LinkState> {
    const stored = await this.state.storage.get<LinkState>("state");
    return stored ?? __zeroOfLinkState();   // a fresh code starts from zero
  }

  async register(url: Url, deps: {}): globalThis.Promise<Result<void, LinkError>> {
    const __state = { ...(await this.loadState()) };   // a mutable working copy
    const __result = await (async () => {
      switch (__state.target.tag) {
        case "Some": {
          return Err(LinkError.AlreadyExists);
        }
        case "None": {
          __state.target = Some(url);                   // `:=` stages the write
          return Ok(undefined);
        }
      }
      throw new globalThis.Error("non-exhaustive match");
    })();
    await this.commitState(__state);                    // one commit at the end
    return __result;
  }
}
```

That `?? __zeroOfLinkState()` is fresh-state initialisation in action: a code with
no stored state falls back to the zero value (`target: None`). The `store` fields
*are* the agent's state record, staged in `__state` and flushed once by
`commitState`. On the `workers` target the same agent compiles to a Cloudflare
Durable Object instead, but the handler logic you wrote is identical.

## Wire it into the API

Now the API can do real work: the handlers store and resolve through the
`Link` agent. The `CodeGen` capability from
[Reach out with capabilities](/book/tutorials/capabilities/) still mints each
code, and `Logger` records each link once it is stored.

The handlers also reuse what [Handle failure with `Result`](/book/tutorials/failure-and-result/) built. `newLink` checks the target and
validates the code, `failure` turns any `LinkError` into a response, and the
agent's own errors are `LinkError`s too, so they go through the same function:

```bynk,fragment
on POST("/links") (
	body: CreateLinkRequest
) -> Effect[HttpResult[CreatedView]] by Visitor given CodeGen, Logger {
	let raw <- CodeGen.next()
	let Ok(view) = newLink(raw, body.target) else match {
		Err(error) => failure[CreatedView](error),
	}
	let registered <- Link(view.code).register(view.target)
	match registered {
		Ok(_) => {
			let _ <- Logger.info("created \(view.code)")
			Created(view)
		},
		Err(error) => failure[CreatedView](error),
	}
}
```

`let Ok(view) = … else match { … }` keeps the good path flat: when `newLink`
fails, the `else match` works out the answer from the error, and the handler
ends there. A link whose code is already taken is answered by `failure` with
`409 Conflict`, as [Handle failure with `Result`](/book/tutorials/failure-and-result/) decided.

The `create` service does the same work for callers inside the context, and
passes `register`'s error straight on with `?`. Its value is `()`, so it is
bound to `_`: `let _ = registered?`. Bynk won't let `registered?` stand alone
as a statement, because a value it produces would be silently thrown away
(`bynk.parse.bare_expression_statement`), so `let _ =` says you meant to.

## The whole file

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

agent Link {
	key code: ShortCode

	store target: Cell[Option[Url]]
	store hits: Cell[Int]

	on call register(url: Url) -> Effect[Result[(), LinkError]] {
		match target {
			Some(_) => Err(AlreadyExists),
			None => {
				target := Some(url)
				Ok(())
			},
		}
	}

	on call resolve() -> Effect[Result[ResolveView, LinkError]] {
		let Some(url) = target else { Err(NotFound) }
		let next = hits + 1
		hits := next
		Ok(ResolveView { target: url, hits: next })
	}
}

service create {
	on call(target: Url) -> Effect[Result[ShortCode, LinkError]] given CodeGen {
		let raw <- CodeGen.next()
		let view = newLink(raw, target)?
		let registered <- Link(view.code).register(view.target)
		let _ = registered?
		Ok(view.code)
	}
}

service api from http {
	on POST("/links") (
		body: CreateLinkRequest
	) -> Effect[HttpResult[CreatedView]] by Visitor given CodeGen, Logger {
		let raw <- CodeGen.next()
		let Ok(view) = newLink(raw, body.target) else match {
			Err(error) => failure[CreatedView](error),
		}
		let registered <- Link(view.code).register(view.target)
		match registered {
			Ok(_) => {
				let _ <- Logger.info("created \(view.code)")
				Created(view)
			},
			Err(error) => failure[CreatedView](error),
		}
	}

	on GET("/links/:code") (code: ShortCode) -> Effect[HttpResult[ResolveView]] by Visitor {
		let resolved <- Link(code).resolve()
		match resolved {
			Ok(view) => Ok(view),
			Err(error) => failure[ResolveView](error),
		}
	}
}
```

```sh
bynk compile . --output out --target workers
```

The shortener now creates real links and resolves them, counting hits as it goes.

## What you have done

You gave the shortener a memory: a `Link` agent keyed by `ShortCode`, with
zeroable `store` fields (`target: Cell[Option[Url]]`, `hits: Cell[Int]`), handlers
that read by bare name and write with `:=`, and an API wired to store and resolve.
You saw fresh-state initialisation and the single end-of-handler commit in the
emitted code. And the agent's failures went through the same `failure` function
as the rest, so every handler stays short.

Everything lives in one context so far. Next, we split the HTTP edge from the
part that keeps the links.

➡️ **Next: [Split into contexts](/book/tutorials/contexts/)**

---

*For what an agent really is and why state must be zeroable, see
[The agent model](/book/concepts/agents-and-state/the-agent-model/). For exact rules, see the
[agents reference](/reference/agents/). For capabilities and providers, see
the [how-to guides](/book/guides/effects-and-capabilities/).*
