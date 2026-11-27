---
title: Capabilities & providers
---
A **capability** is a typed interface to the outside world; a **provider**
implements one. Handlers receive capabilities through a `given` clause. All three
live inside a `context`.

## Declaring a capability

```bynk
capability Logger {
  fn info(message: String) -> Effect[()]
}
```

A capability is a set of operation *signatures* (no bodies). Each operation
returns `Effect[T]` (capabilities are how effectful work reaches the outside).

## Providing a capability

```bynk,fragment
provides Logger = ConsoleLogger {
  fn info(message: String) -> Effect[()] {
    Effect.pure(())
  }
}
```

`provides Cap = Impl { … }` implements every operation of `Cap`. The signatures
must match exactly (`bynk.provider.signature_mismatch`,
`bynk.provider.missing_operation`, `bynk.provider.extra_operation`). There is one
provider per capability in a context.

A capability a context declares and requires with `given` needs a provider in
that context when you build for Workers: a Worker's composition root builds
every capability its handlers are given (`bynk.capability.not_provided`). On the
bundle target a capability with no provider is left to the host, which supplies
it through `__makeSurface(deps)`, unless the bundle has a composition root
(`composeApp`). A bundle has one once any context consumes another context's
service or capability, emits events, or derives a capability, and `composeApp`
builds every context itself, so each required capability then needs a provider
there too. A test `stub` doesn't count: it replaces the provider only in the
cases that stub it.

## Using a capability

A handler lists the capabilities it needs with `given`, then calls them:

```bynk,fragment
service hello {
  on call() -> Effect[String] by Caller given Logger {
    let _ <- Logger.info("hi")
    "ok"
  }
}
```

A `given` name must be a declared capability (`bynk.given.unknown_capability`); a
call to a capability not in `given` is an error (`bynk.given.undeclared_capability`);
a declared-but-unused capability is a warning (`bynk.given.unused_capability`).

## Capabilities in functions {#capabilities-in-functions}

A **free function in a context** may declare `given` after its return type, and use
those capabilities in its body (a lambda there included):

```bynk,fragment
fn warn(what: String) -> Effect[()] given Logger {
  Logger.error("warning: \(what)")
}
```

- **Where.** Only a free function that returns `Effect[T]`, in a context, a commons
  ([capability parameters](#capability-parameters), below) or a suite (a helper's `given`
  names the target's capabilities). An adapter's helpers are pure, a method cannot, and
  `Events` cannot be named (`bynk.given.fn_unsupported`).
- **Calling it.** The caller's `given` MUST include the callee's. A call, or a reference
  to the function as a value, from a body that lacks one is
  `bynk.given.missing_for_call`, naming the capabilities missing. This is checked at
  every call, so it holds transitively; recursion needs no special case. A call counts
  as a use of the callee's capabilities in the caller's `given`.
- **Tests.** A `case` may call it directly and gets the target's capabilities as its
  handlers do. `stub` and `expect … called` observe the calls made inside it.
- **Generics.** The function may be generic. Capability operations keep their own type
  arguments.
- **Emission.** The function takes a hidden trailing `deps` parameter that carries
  exactly the capabilities it names (before the execution context of
  [`detach`, `within` and `race`](#within)), and every call passes the caller's `deps` on.

## Capability parameters in a commons {#capability-parameters}

:::caution[Experimental]
Capability parameters, the binding at a call, a capability in a commons' `consumes`
and a commons suite's suite-local instances are experimental. A project opts in with
`[unstable] allow = ["bynk.given.parameters"]`; without it each is
`bynk.stability.experimental`.
:::

A commons is shared by contexts that hold different capabilities, and most
`bynk.cloudflare` kinds (`D1`, `R2`, `Queue`, `Service`, …) are used only through a
context's own derived instances. A **free function in a commons** therefore takes
the instance it needs as a **capability parameter**, and every call names it. The
guide [Write a library that uses a capability](/book/guides/effects-and-capabilities/write-a-library-that-uses-a-capability/)
walks through one from declaration to tests:

```bynk,fragment
commons shop.cache

consumes bynk { Logger }
consumes bynk.cloudflare { Kv }

fn remember(key: String, value: String) -> Effect[String] given Store: Kv, Logger {
	let hit <- Store.get(key)
	match hit {
		Some(text) => Effect.pure(text),
		None => {
			let _ <- Store.put(key, value)
			let _ <- Logger.info("stored \(key)")
			Effect.pure(value)
		},
	}
}
```

```bynk,fragment
context shop

uses shop.cache
consumes bynk { Logger }
consumes bynk.cloudflare { Kv }

capability Sessions = Kv

fn session(id: String) -> Effect[String] given Sessions, Logger {
	remember(id, "fresh", given Store = Sessions)
}
```

- **Selecting a kind.** A commons names a first-party capability by listing it in its
  braced `consumes`, beside any types: `consumes bynk.cloudflare { Kv, KvEntry }`. The
  selection imports the contract only: nothing is provided or bound, and a unit that
  `uses` the commons gets no capability through it. A capability of a project's own
  adapter is still `bynk.commons.consumes_not_types`, and one that no function's `given`
  names is the warning `bynk.commons.capability_unused`. Naming a `bynk` capability is an
  explicit `consumes bynk`, so the file then lists every `bynk` type it names too.
- **Parameter or shared.** An **instantiable** kind (one a context may declare instances
  of: `Kv`, `D1`, `R2`, `Queue`, `Service`, …) is always a parameter, `given Store: Kv`;
  named bare it is `bynk.given.kind_needs_parameter`, so a library never silently means
  "the caller's default instance". Any other first-party capability (`Clock`, `Random`,
  `Logger`, `Fetch`, `Secrets`, `Locale`, `Idempotency`, the Cloudflare singletons,
  `Crypto`) is **shared** and named bare, `given Logger`: it means the caller's one, and
  the caller's `given` must hold it, as between context functions. As a bound it is
  `bynk.given.shared_as_parameter`. `Events` stays refused.
- **Naming.** A parameter is capitalised and is called like a capability
  (`Store.get(key)`), lambdas in the body included. It may not shadow a capability or kind
  the commons selects, a type parameter, or another parameter
  (`bynk.given.parameter_name_conflict`), and it is never a value. Only a commons free
  function, a test-only commons' included, takes parameters: anywhere else `given Name:
  Kind` is `bynk.given.parameter_outside_commons`.
- **Binding.** Every call binds every parameter, inside its argument list after the
  ordinary arguments, behind `given`: `f(x, given Store = Sessions)`, or `f(given Store =
  Sessions)` with no other arguments. The binding is part of the call, so it works in a
  `<-`, `do`, `par`, a lambda and a method chain alike. Each parameter is bound exactly
  once, in any order; a missing, unknown or repeated one is `bynk.given.binding_missing`,
  `bynk.given.binding_unknown` or `bynk.given.binding_duplicate`, and a shared capability
  is never bound (`bynk.given.binding_shared`).
- **What may be bound.** The right-hand side is a capability in the caller's own `given`
  (`bynk.given.missing_for_call` otherwise) of exactly the parameter's kind
  (`bynk.given.binding_kind_mismatch` otherwise): a derived instance of it, the bare kind
  where it allows a bare use (only `Kv` today: `given Store = Kv`, the caller's choice,
  written out), or in a commons the caller's own parameter of that kind. One instance may
  bind two parameters; the aliasing is visible at the call. A Bynk-bodied provider may
  call a commons function and bind its own `given` capabilities.
- **Forwarding.** A commons function calling another binds the callee's parameters to
  its own, `middle(key, xs, given Mid = Store)`: the left name is the callee's, the right
  the caller's. A commons body sees no instance, so it can bind only to its own
  parameters. The rule is checked at each call, so it holds along a chain beside type
  parameters and `Ord` bounds, through recursion (a recursive call may rebind its
  parameters, `swap(n - 1, given A = B, B = A)`), and a binding missing deep in a chain is
  reported at the function that leaves it out.
- **Not a value.** A function with capability parameters cannot be named as a value
  (`bynk.given.parameterised_fn_value`): a value would leave its bindings unchosen. Write a
  lambda that binds instead, `keys.forEach((k) => evict(k, given Inner = Store))`. A
  function with only shared capabilities may still be named as a value.
- **Testing.** A test names capabilities, never parameters. In a context's suite a call
  through `Store` reaches the bound instance, so the case stubs and observes that one:
  `stub Sessions.get(_) returns None`, `expect Sessions.put called once`. A commons suite
  holds a double of every capability its commons selects, named by the kind once the suite
  file `consumes` it (`consumes bynk.cloudflare { Kv }`): a case binds a parameter to it,
  `remember(key, given Store = Kv)`, and stubs and observes `Kv`; a shared capability's
  double is observed the same way (`expect Logger.info called`). Each double is fresh per
  case. For a second double of one kind, a commons suite declares a suite-local instance,
  `capability Archive = Kv`, with no binding and no deploy effect. It is shared by every
  suite of the commons, so it is declared once (`bynk.resolve.duplicate_capability`
  otherwise) and named from any of them. A context's suite cannot
  (`bynk.capability.derived_outside_context`), nor can a commons itself. A `given` suite
  helper names the doubles it uses (`fn seed() -> Effect[()] given Kv`), and setup over a
  parameter lives in a test-only commons, which a case calls with a binding. Naming a
  parameter in a `stub` or an `expect … called` is `bynk.stub.not_a_seam` or
  `bynk.observe.not_a_seam`, with a note naming what to stub instead.
- **Never inferred.** A binding is written at every call, even when the caller holds only
  one candidate, because reading the wrong instance is well-typed and nothing else would
  catch it. The fix for `bynk.given.binding_missing` offers one binding per instance of
  the kind the caller holds, and is applied by `bynk check --fix` only when there is
  exactly one.
- **Emission.** The function takes the same hidden `deps` as a context function, typed by
  parameter and shared names (`deps: { Store: Kv; Logger: Logger }`), and a call builds it
  from the caller's: `remember(id, "fresh", { Store: deps.Sessions, Logger: deps.Logger })`
  (`this.deps` in a provider). It costs one object per call, and no runtime or binding
  changes. In a test build `deps.Sessions` is already the recording proxy, so `expect
  Sessions.put called` observes a call made through `Store`.

## Joining independent effects — `par` {#par}

Every `<-` waits for its effect, so two independent calls cost the sum of their
latencies. A `par` block issues several effects together and waits for all of them:

```bynk,fragment
service dashboard {
  on call(id: Int) -> Effect[String] by Caller given Users, Orders {
    par {
      let user <- Users.get(id)
      let orders <- Orders.forUser(id)
    }
    Effect.pure("\(user): \(orders)")
  }
}
```

Both effects are issued, in source order, before either is awaited. The block
completes when every binding has settled, and the names it binds are in scope after
the closing brace, not inside it. `par` is contextual: it opens a statement only
before `{`, and is an ordinary identifier everywhere else.

A `par` block holds only `let x <- e` (optionally `let x: T <- e`) and `do e`, and is
legal wherever a `<-` is: an effectful body, an agent handler included. It has no
value; it is a statement. These are refused:

| In the block | Code | Why |
|---|---|---|
| a plain `let`, `:=` or a `store` write, `expect`, `advance`, a nested `par`, a `let … else`, or nothing at all | `bynk.par.invalid_statement` | each has no effect to overlap, nothing to join, or must stay sequential |
| `?` in a binding | `bynk.par.question_in_join` | `?` takes a `Result`, not an effect: bind the `Result` in the block and apply `?` after it (`let user = userR?`), where the first `Err` in source order propagates; compute a `?` on an argument before the block |
| a binding that mentions a name another binding of the block binds, or two bindings of one name | `bynk.par.sibling_name` | the bindings run together, so none can read a sibling's result; the rule holds even where the name would resolve to an outer binding |

A block with one binding is the warning `bynk.par.single_binding`: it is a plain
`let x <- e` with extra syntax.

**Store writes stay sequential in `parForEach` and `parTraverse` too.** The closure given to
`parForEach`, `parTraverse`, `parTraverseAll` or `parTraverseTry` (and their bounded `…With` forms, which take a `ParOptions` from `bynk`) runs once per element, all at
once (or `limit` at a time), so it follows the same rule: it may not write a `store` field
(`bynk.par.store_write_in_closure`). That covers `put`, `remove`, `update`, `upsert`,
`add`, `append` and `:=`, in the closure, in a lambda nested in it, or in a lambda
bound to a name and passed by name. Reads, a store used as the *receiver*
(`conns.parForEach(…)`), and calls to other agents are fine. A `get` then a `put` in
a closure loses updates, because every `<-` is an await point and the branches
interleave between the two; run the loop with `forEach` instead, where a single-call
`update` or `upsert` is atomic per key.

**If an effect faults,** the block waits for the others to settle, then raises the
**first fault in source order**, not the first in time. A fast failure waits for a
slow sibling; that is what keeps every effect inside the handler's lifetime and the
fault a program sees independent of timing. Effects that completed before the fault
keep their effects, exactly as in a sequence of `<-`s. See
[what a join guarantees on failure](/book/concepts/effects-and-capabilities/what-a-join-guarantees-on-failure/).

## Bounding an effect — `within` {#within}

`e.within(d)` bounds how long an effect may take. It is a method of every
`Effect[T]` (the others, `map`, `andThen`, `race` and `detach`, are in
[Effect methods](/reference/types/#effect-methods)), `d` is a `Duration`, and the
result is `Effect[Result[T, TimedOut]]`:

```bynk,fragment
// the unit has: consumes bynk { TimedOut }
let r <- Pricing.quote(id).within(2.seconds)
match r {
  Ok(q) => Effect.pure("quoted \(q)")
  Err(TimedOut { after, .. }) => Effect.pure("late")   // the quote may still complete
}
```

`TimedOut { after: Duration }` is `bynk`'s, so the unit needs `consumes bynk { TimedOut }`
(or `TimedOut` in its existing `consumes bynk { … }` list); without it the call is
`bynk.within.timed_out_not_in_scope`. When the
deadline passes first, the effect is **not cancelled**: it is handed to the
platform's `waitUntil` and may still complete after the handler returns, with its side
effects. A tie goes to the effect. `FetchOptions.timeout` is the tool when the work must
actually stop. An effect that already returns a `Result` gains a second layer.

| Where | Code | Why |
|---|---|---|
| an agent handler, a lambda in one included | `bynk.within.in_agent_handler` | no execution context to hand the abandoned effect to |
| a provider operation, a WebSocket handler or a test `case` body | `bynk.within.outside_handler` | none of them has an execution context (a WebSocket handler runs in the connection's Durable Object). An effectful `fn` or method is allowed: it takes the context from its caller (#2719) |
| a call to, or a reference to, a function that uses `within`, `race` or `detach`, from an agent handler, a WebSocket handler or a provider operation | `bynk.exec.unavailable` | the note names the chain of calls down to the construct |
| a pure body | `bynk.within.in_pure_context` | it needs an effectful body |
| a unit that does not consume `bynk` | `bynk.within.timed_out_not_in_scope` | `TimedOut` is `bynk`'s |

A deadline that is not over zero and at most 2,147,483,647 ms faults the call. See
[bound how long a call may take](/book/guides/effects-and-capabilities/bound-how-long-a-call-may-take/).

## Racing two effects — `race` {#race}

`a.race(b)` takes whichever of two effects succeeds first. It is a method of every
`Effect[T]`, `b` is an `Effect[T]` too, and the result is `Effect[T]`. It chains:
`a.race(b).race(c)` is the first of three.

```bynk,fragment
let p <- Prices.get("east", sku).race(Prices.get("west", sku))
```

- **First to succeed.** A side that faults does not win while another may still
  succeed. If every side faults, the first fault in source order is raised, with the
  others attached as `suppressed`. An `Err` returned by an `Effect[Result[T, E]]` is a
  value, so it is a success and can win.
- **A tie goes to the receiver:** the side written first.
- **The loser is not cancelled.** It is handed to the platform's `waitUntil` and runs to
  completion, so its side effects still happen. `race` is for idempotent work, and the
  work is done twice. `FetchOptions.timeout` is the tool when the work must stop.
- A mismatched `T` is an ordinary type error; map each side into a shared type first.

It is legal exactly where [`within`](#within) is, and refused in the same places:

| Where | Code | Why |
|---|---|---|
| an agent handler, a lambda in one included | `bynk.race.in_agent_handler` | no execution context to hand the loser to |
| a provider operation, a WebSocket handler or a test `case` body | `bynk.race.outside_handler` | none of them has an execution context. An effectful `fn` or method is allowed (#2719), as for `within` |
| a pure body | `bynk.race.in_pure_context` | it needs an effectful body |

See [ask two sources and take the first answer](/book/guides/effects-and-capabilities/ask-two-sources-and-take-the-first-answer/).

## Detaching an effect — `detach` {#detach}

`e.detach()` on any `Effect[T]` stops waiting for `e`. It returns an `Effect[()]` that completes at
once: **the effect is already running** (an effect is an eager promise, started where the expression
is evaluated), and `detach` hands that running promise to the platform's `waitUntil` so it can finish
after the handler has answered. It never starts anything and never delays one.

```bynk,ignore
fn notify(req: Request) -> Effect[()] given Fetch {
  Fetch.send(req).detach()        // an `Err` reply is reported, never lost
}

fn notifyAll(reqs: List[Request]) -> Effect[()] given Fetch {
  reqs.forEach((r) => Fetch.send(r).detach())
}
```

A detached reply is never silently lost. A failure is reported by the runtime, not by `Logger`:

- **A fault** (the effect's promise rejects) is passed to the platform's exception path unchanged, so
  the platform records an exception, labelled with the detach site.
- **An `Err`** (when `T` is a `Result`) is one structured `console.error` record naming the site, the
  error's type and, for a sum, its variant tag. The payload is not reported, because it can hold a
  credential.
- `Ok`, and any `T` that is not a `Result`, report nothing.

Detaching is immediate and best-effort: it is not retried. On Workers, `waitUntil` work can run for up to
30 seconds after the response is sent or the client disconnects, shared across the request; promises still
unsettled then are cancelled (local `workerd` does not enforce this). In a test, the harness records each report and does not fail the case, as on
Workers.

It is legal exactly where [`within`](#within) is, and an agent handler or a pure body is refused:

| Where | Code | Why |
|---|---|---|
| an agent handler, a lambda in one included | `bynk.detach.in_agent_handler` | no execution context to keep the effect alive |
| a store operation in an agent handler | `bynk.detach.store_op` | a store write is part of the handler's atomic commit |
| a pure body | `bynk.detach.in_pure_context` | it needs an effectful body |

## Generic operations

A capability operation may declare its own type parameter (v0.235, ADR 0281) —
useful when the value it hands back is of whatever type the *calling* handler
determines, not a type the capability's author can know ahead of time. The
example below is also the real shape of the first-party `Idempotency`
capability this mechanism was built for — see
[First-party `bynk` capabilities](/reference/bynk-capabilities/#the-idempotency-capability)
for its actual, shipped two-operation form (`dedup` plus a `remember` to write
a value back):

```bynk
capability Idempotency {
  fn dedup[T](key: String) -> Effect[Option[T]]
}
```

`T` is resolved from an **explicit** call-site type argument, or inferred from the
arguments when they determine it (`Idempotency.remember(key, value, ttl)`, `Events.emit(event)`).
`dedup` has no argument that mentions `T`, so it is written, as `Json.decode[T](s)` is, and
`T` is never inferred from the expected type:

```bynk,fragment
service reserve {
  on call() -> Effect[Option[ReserveOutcome]] by Caller given Idempotency {
    let cached <- Idempotency.dedup[ReserveOutcome]("order-key-1")
    cached
  }
}
```

Written type arguments bind the operation's **leading** type parameters, and the arguments
determine the rest: for `fn dedup[T: Codable, K: Key](key: K)`, `Idempotency.dedup[Quote](id)`
names `T` and takes `K` from `id` (#4116). Omitting a type argument that no argument determines is
`bynk.generics.uninferable_type_arg`; more type arguments than parameters is
`bynk.generics.type_arg_mismatch`. A qualified cross-context call (`B.Cap.op(…)`) infers a
left-out type parameter only from an argument whose parameter is exactly that parameter
(`key: K`). The operation is emitted as a genuine
generic TypeScript interface method (`dedup<T>(key: string): Promise<Option<T>>`)
— no monomorphisation, no erasure — and the call site names the type argument
explicitly in the emitted TypeScript too, since nothing else at the call site
lets `tsc` infer a return-position-only parameter.

Two restrictions follow directly from there being no runtime codec to
specialise a generic operation's `T` against:

- **A capability with a generic operation requires an [external
  provider](/reference/adapters/)** (`bynk.provider.generic_op_requires_external`)
  — a Bynk-bodied provider would need `T` rigid through its own body checker
  for no expressive gain, since it can only ever return a value it already
  has. This means a capability with a generic operation can only be declared
  inside an `adapter`, never a plain `context` (whose provider must have a
  Bynk body).
- **A `stub` of a generic operation names its type argument**
  (`stub Idempotency.dedup[Int](_) returns None`). Writing the type argument the
  way a call site does makes the clause typed: its arguments and its `returns`
  value are checked against the operation with `T` replaced by `Int`, and the
  clause answers only the calls made at `Int`. A call at another type argument
  finds no matching clause, as a call with an unmatched argument does. A `stub`
  without the type argument is refused (`bynk.stub.generic_op`), as is one that
  gives the wrong number of them (`bynk.generics.type_arg_mismatch`). Stubbing
  another, non-generic operation of the same capability is unaffected. See
  [`stub`](/reference/testing/#stub).

A generic operation is called the same way through a cross-context capability,
both flattened (`consumes Adapter { Cap }` → `Cap.op[T](…)`) and qualified
(`consumes Adapter` → `Adapter.Cap.op[T](…)`).

## Provider composition (`provides … given`)

A provider may itself depend on other capabilities — declare them with `given`
after the provider name, and call them in the bodies:

```bynk
context demo

capability Logger {
	fn info(message: String) -> Effect[()]
}

capability Greeter {
	fn greet() -> Effect[()]
}

provides Logger = ConsoleLogger {
	fn info(message: String) -> Effect[()] {
		Effect.pure(())
	}
}

provides Greeter = PoliteGreeter given Logger {
	fn greet() -> Effect[()] {
		let _ <- Logger.info("hello")
		Effect.pure(())
	}
}
```

The same `given` discipline applies (unknown / undeclared-use are errors). The
providers form a **dependency graph** over capabilities; the composition root
instantiates them in dependency order, injecting each provider's dependencies.

A capability may not depend on itself, directly or transitively
(`bynk.provider.dependency_cycle`) — including the trivial `provides X = … given
X`.

## Derived capabilities (`capability X = K`) {#derived-capabilities}

A **derived** capability is a distinct instance of a platform resource, such as
a second KV namespace:

```bynk
context shop.cart

consumes bynk.cloudflare { Kv }

capability Sessions = Kv

capability PriceCache = Kv

service remember {
	on call(k: String, v: String) -> Effect[Option[String]] by Caller given Sessions {
		let _ <- Sessions.put(k, v)
		Sessions.get(k)
	}
}
```

`Sessions` has `Kv`'s operations, but its own storage. `PriceCache` is another
instance, and the bare `Kv`, if used, a third. It is not an alias: a key written
through `Sessions` is absent through `PriceCache`, just as `type Id = Int` is a
distinct type over `Int`. Write no `provides`. The base's platform provider
serves each instance, bound to the instance's own resource. In every other way
an instance is an ordinary capability: `given`, calls, `stub` and `expect` work
on it unchanged.

- The base must be a capability the context selects with `consumes U { K }`
  (`bynk.capability.unknown_base`), and a resource kind with instances
  (`bynk.capability.base_not_instantiable`). Today that is `bynk.cloudflare`'s
  `Kv`, `Service`, `R2`, `D1`, `RateLimit`, `Queue`, `Analytics`, `EdgeCache`,
  `StoredSecret`, `Email`, `Postgres` and `MySql`. `Version` is not: a Worker has one Version metadata binding, so it is used
  bare, with `given Version`.
- Options follow the base as `@annotations`, from the kind's own set
  (`bynk.capability.unknown_option`), each one literal of the option's kind,
  given once (`bynk.capability.invalid_option`). An option also takes a
  [constant](/reference/constants/) declared exactly the option's type and bound
  to a literal, or to another such constant (`@worker(BillingWorker)`); a list
  option such as `@allowedRecipients` takes constants for its string elements.
  The constant is replaced by its literal before the option is checked, so every
  rule below applies to its value, and a constant that does not qualify keeps the
  option's own error. `Kv` takes none. `Service`
  requires `@worker("name")`, the Worker it reaches
  (`bynk.capability.missing_option`), which must not be one of the project's own
  contexts' Workers. `R2` takes an optional `@jurisdiction("eu")`. `RateLimit`
  requires `@limit(n)`, a positive `Int`, and `@period(d)`, `10.seconds` or
  `60.seconds` ([Rate limiting](/reference/bynk-capabilities/#rate-limiting-ratelimit)).
  `Queue` requires `@name("queue")`, the queue it sends to
  ([Sending to a queue](/reference/bynk-capabilities/#sending-to-a-queue-queue)).
  `Analytics` takes none: `bynk deploy` names its dataset
  ([Writing events](/reference/bynk-capabilities/#writing-events-analytics)).
  `D1` takes none: `bynk deploy` names and creates its database
  ([SQL databases](/reference/bynk-capabilities/#sql-databases-d1)).
  `StoredSecret` requires `@store("name")` and `@secret("name")`, the Secrets
  Store and the secret in it, which `bynk deploy` links and never creates
  ([Account secrets](/reference/bynk-capabilities/#account-secrets-storedsecret)).
  `Email` takes `@to("addr")`, `@allowedRecipients([…])` and
  `@allowedSenders([…])`, each optional: deploy-time policy written into
  `[[send_email]]`, whose senders `bynk deploy` checks and never configures
  ([Outbound email](/reference/bynk-capabilities/#outbound-email-email)).
  `Postgres` takes none: it binds as `HYPERDRIVE_<NAME>`, a `[[hyperdrive]]`
  entry whose `id` is a placeholder `bynk deploy` replaces with the Hyperdrive
  configuration it makes from a connection string in an environment variable
  `bynk.toml` names ([Postgres databases](/reference/bynk-capabilities/#postgres-databases-postgres)).
  `MySql` takes an optional `@tls("required")`, `"verify-ca"` or `"verify-identity"`
  (how Hyperdrive secures its connection to the database; anything else is
  `bynk.capability.invalid_option`), and binds as `HYPERDRIVE_<NAME>` like
  `Postgres` ([MySQL databases](/reference/bynk-capabilities/#mysql-databases-mysql)).
  `EdgeCache` takes none, and a context declares at most one
  (`bynk.cloudflare.edge_cache_duplicate`): it turns on the Worker's edge cache
  and has no binding ([Edge caching](/reference/bynk-capabilities/#edge-caching-edgecache)).
  `Service`, `R2`, `RateLimit`, `Queue`, `Analytics`, `D1`, `Postgres`, `MySql` and `EdgeCache` are used only through instances
  (`bynk.capability.instance_only`).
- Only a context declares one (`bynk.capability.derived_outside_context`), and
  it cannot be exported (`bynk.exports.derived_capability`). The data belongs to
  the context, so another context reaches it through the context's services.

On Workers, each instance gets its own binding, named from the kind and the
capability (`Sessions` → `KV_SESSIONS`, `PriceCache` → `KV_PRICE_CACHE`), with an
`Env` member and a `[[kv_namespaces]]` stanza of its own. The bare `KV` stanza is
emitted only for a context that uses the bare `Kv`. Under `bynk test`, each
instance has its own fresh in-memory double. `bynk deploy` provisions each
instance's namespace and records its id, titled `<worker>-<capability>`
([Several KV namespaces](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#several-kv-namespaces)). See [Use more than one KV namespace](/book/guides/effects-and-capabilities/use-several-kv-namespaces/).

A `Service` instance (`capability Billing = Service @worker("legacy-billing")`)
binds as `SERVICE_BILLING`, a `[[services]]` entry naming its Worker. Under
`bynk test` its double faults until `send` is stubbed; `bynk deploy` binds it
without provisioning anything. See [Call a Worker outside the project](/book/guides/effects-and-capabilities/call-a-foreign-worker/).

## Cross-context capabilities (`exports capability`)

A context can offer a capability for *other* contexts to consume — the pattern
behind **platform / framework contexts** (a `Clock`, an `Http` client, a
`Random` source) that application contexts depend on without re-declaring.

The providing context lists the capability in an `exports capability { … }`
clause; each name must be a capability the context both **declares** and
**provides**:

```bynk
context platform.time

exports capability { Clock }

capability Clock {
	fn now() -> Effect[Int]
}

provides Clock = SystemClock {
	fn now() -> Effect[Int] {
		0
	}
}
```

A consumer `consumes` that context and depends on the capability through a
**qualified `given`** — `given B.Cap`, or `given Alias.Cap` when the `consumes`
clause introduces an alias. The capability call uses the same prefix:

```bynk,fragment
context ops.jobs

consumes platform.time

service tick {
	on call() -> Effect[Int] by Caller given platform.time.Clock {
		let t <- platform.time.Clock.now()
		t
	}
}
```

The capability **contract** is imported for type-checking; the **provider** is
instantiated in the consumer's own composition and the call runs **in-process**
(no Worker hop) — each consuming Worker gets its own provider instance, exactly
as platform capabilities intend. A consumer's provider may also depend on a
cross-context capability (`provides X = Impl given B.Cap`); the composition root
wires the provider across the boundary.

Errors:

- `bynk.exports.undeclared_capability` — `exports capability` names something the
  context does not declare as a capability.
- `bynk.exports.capability_not_provided` — an exported capability has no provider
  (a consumer could not instantiate it).
- `bynk.given.cross_context_unknown_capability` — `given B.Cap` where `B` does
  not export `Cap`.
- A `given B.Cap` whose `B` is not `consumes`-d is the ordinary
  `bynk.resolve.unconsumed_context`.

Out of scope (deferred): remote routing of capability calls to the providing
Worker, capabilities backed by another context's private agent state, and
transitive re-export of a consumed capability.

## Emission

Providers compile to classes implementing the capability interface; a composed
provider gains a constructor that receives its dependencies, and the generated
`compose` instantiates providers in topological order. A cross-context
capability is instantiated locally in the consumer's composition (its provider
class imported from the providing context), so the call lowers to an ordinary
`deps.<Cap>.op(…)`. See [emission](/reference/tooling/emission/).
