---
title: Add a stateful agent
---
Everything so far has been stateless: a request comes in, a value goes out,
nothing is remembered. A URL shortener has to remember — which code maps to which
URL, and how often each was followed. In Bynk, the unit of state is an
**[agent](/book/reference/glossary/#term-agent)**:
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
  [Tutorial 4](/book/tutorials/04-refined-types/) means a link can only ever be keyed by a
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
    match target {
      Some(url) => {
        let next = hits + 1
        hits := next
        Ok(ResolveView { target: url, hits: next })
      }
      None => Err(NotFound)
    }
  }
```

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

Now the API can do real work. A small `CodeGen` capability mints new codes, and
the handlers store and resolve through the `Link` agent:

```bynk
capability CodeGen {
  fn next() -> Effect[String]
}

provides CodeGen = FixedCodeGen {
  fn next() -> Effect[String] {
    "abc123"
  }
}
```

A **capability** is a dependency a handler asks for with `given`; a **provider**
supplies it. They are a topic in their own right — see
[Compose a provider](/book/guides/effects-and-capabilities/compose-a-provider/) — but the shape
above is all we need: mint a raw string, then validate it into a `ShortCode`.

## The whole file

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

capability CodeGen {
  fn next() -> Effect[String]
}

provides CodeGen = FixedCodeGen {
  fn next() -> Effect[String] {
    "abc123"
  }
}

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

  on call resolve() -> Effect[Result[ResolveView, LinkError]] {
    match target {
      Some(url) => {
        let next = hits + 1
        hits := next
        Ok(ResolveView { target: url, hits: next })
      }
      None => Err(NotFound)
    }
  }
}

service create {
  on call(target: Url) -> Effect[Result[ShortCode, LinkError]] given CodeGen {
    let raw <- CodeGen.next()
    match ShortCode.of(raw) {
      Err(_) => Err(Invalid)
      Ok(code) => {
        let link = Link(code)
        let outcome <- link.register(target)
        match outcome {
          Ok(_) => Ok(code)
          Err(e) => Err(e)
        }
      }
    }
  }
}

service api from http {
  on POST("/links") (body: CreateLinkRequest) -> Effect[HttpResult[CreatedView]] by Visitor given CodeGen {
    let raw <- CodeGen.next()
    match ShortCode.of(raw) {
      Err(_) => ServerError("generated an invalid code")
      Ok(code) => {
        let link = Link(code)
        let outcome <- link.register(body.target)
        match outcome {
          Ok(_) => Created(CreatedView { code: code, target: body.target })
          Err(linkError) => match linkError {
            AlreadyExists => Conflict("code already in use")
            NotFound => ServerError("unexpected state")
            Invalid => ServerError("invalid code")
          }
        }
      }
    }
  }

  on GET("/links/:code") (code: ShortCode) -> Effect[HttpResult[ResolveView]] by Visitor {
    let link = Link(code)
    let outcome <- link.resolve()
    match outcome {
      Ok(view) => Ok(view)
      Err(linkError) => match linkError {
        NotFound => NotFound
        AlreadyExists => ServerError("unexpected state")
        Invalid => ServerError("invalid code")
      }
    }
  }
}
```

```sh
bynkc compile . --output out --target workers
```

The shortener now creates real links and resolves them, counting hits as it goes.

## What you have done

You gave the shortener a memory: a `Link` agent keyed by `ShortCode`, with
zeroable `store` fields (`target: Cell[Option[Url]]`, `hits: Cell[Int]`), handlers
that read by bare name and write with `:=`, and an API wired to store and resolve.
You saw fresh-state initialisation and the single end-of-handler commit in the
emitted code.

We have asserted that all this works — now let us prove it.

➡️ **[Tutorial 6: Test it](/book/tutorials/06-testing/)**

---

*For what an agent really is and why state must be zeroable, see
[The agent model](/book/guides/agents-and-state/the-agent-model/). For exact rules, see the
[agents reference](/book/reference/agents/). For capabilities and providers, see
the [how-to guides](/book/guides/effects-and-capabilities/).*
