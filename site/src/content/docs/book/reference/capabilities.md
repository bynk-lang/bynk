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
it through `__makeSurface(deps)`.

## Using a capability

A handler lists the capabilities it needs with `given`, then calls them:

```bynk,fragment
service hello {
  on call() -> Effect[String] given Logger {
    let _ <- Logger.info("hi")
    "ok"
  }
}
```

A `given` name must be a declared capability (`bynk.given.unknown_capability`); a
call to a capability not in `given` is an error (`bynk.given.undeclared_capability`);
a declared-but-unused capability is a warning (`bynk.given.unused_capability`).

## Generic operations

A capability operation may declare its own type parameter (v0.235, ADR 0281) —
useful when the value it hands back is of whatever type the *calling* handler
determines, not a type the capability's author can know ahead of time. The
example below is also the real shape of the first-party `Idempotency`
capability this mechanism was built for — see
[First-party `bynk` capabilities](/book/reference/bynk-capabilities/#the-idempotency-capability)
for its actual, shipped two-operation form (`dedup` plus a `remember` to write
a value back):

```bynk
capability Idempotency {
  fn dedup[T](key: String) -> Effect[Option[T]]
}
```

`T` is resolved only from an **explicit** call-site type argument — never
inferred from the arguments or the expected type, the same discipline
`Json.decode[T](s)` uses:

```bynk,fragment
service reserve {
  on call() -> Effect[Option[ReserveOutcome]] given Idempotency {
    let cached <- Idempotency.dedup[ReserveOutcome]("order-key-1")
    cached
  }
}
```

Omitting `[T]` when no parameter's type mentions it is
`bynk.generics.uninferable_type_arg`; the wrong number of type arguments is
`bynk.generics.type_arg_mismatch`. The operation is emitted as a genuine
generic TypeScript interface method (`dedup<T>(key: string): Promise<Option<T>>`)
— no monomorphisation, no erasure — and the call site names the type argument
explicitly in the emitted TypeScript too, since nothing else at the call site
lets `tsc` infer a return-position-only parameter.

Two restrictions follow directly from there being no runtime codec to
specialise a generic operation's `T` against:

- **A capability with a generic operation requires an [external
  provider](/book/reference/adapters/)** (`bynk.provider.generic_op_requires_external`)
  — a Bynk-bodied provider would need `T` rigid through its own body checker
  for no expressive gain, since it can only ever return a value it already
  has. This means a capability with a generic operation can only be declared
  inside an `adapter`, never a plain `context` (whose provider must have a
  Bynk body).
- **A generic operation cannot be stubbed in a test** (`bynk.stub.generic_op`)
  — a `stub` clause's body has no way to construct a value of an unconstrained
  `T`. Stubbing another, non-generic operation of the same capability is
  unaffected.

A generic operation is called the same way through a cross-context capability,
both flattened (`consumes Adapter { Cap }` → `Cap.op[T](…)`) and qualified
(`consumes Adapter` → `Adapter.Cap.op[T](…)`).

## Provider composition (`provides … given`)

A provider may itself depend on other capabilities — declare them with `given`
after the provider name, and call them in the bodies:

```bynk
context demo

capability Logger  { fn info(message: String) -> Effect[()] }
capability Greeter { fn greet() -> Effect[()] }

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

## Derived capabilities (`capability X = K`)

A **derived** capability is a distinct instance of a platform resource, such as
a second KV namespace:

```bynk
context shop.cart

consumes bynk.cloudflare { Kv }

capability Sessions = Kv
capability PriceCache = Kv

service remember {
  on call(k: String, v: String) -> Effect[Option[String]] given Sessions {
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
  `Kv`, `Service`, `R2`, `RateLimit`, `Queue`, `Analytics` and `EdgeCache`.
- Options follow the base as `@annotations`, from the kind's own set
  (`bynk.capability.unknown_option`), each one literal of the option's kind,
  given once (`bynk.capability.invalid_option`). `Kv` takes none. `Service`
  requires `@worker("name")`, the Worker it reaches
  (`bynk.capability.missing_option`), which must not be one of the project's own
  contexts' Workers. `R2` takes an optional `@jurisdiction("eu")`. `RateLimit`
  requires `@limit(n)`, a positive `Int`, and `@period(d)`, `10.seconds` or
  `60.seconds` ([Rate limiting](/book/reference/bynk-capabilities/#rate-limiting-ratelimit)).
  `Queue` requires `@name("queue")`, the queue it sends to
  ([Sending to a queue](/book/reference/bynk-capabilities/#sending-to-a-queue-queue)).
  `Analytics` takes none: `bynk deploy` names its dataset
  ([Writing events](/book/reference/bynk-capabilities/#writing-events-analytics)).
  `EdgeCache` takes none, and a context declares at most one
  (`bynk.cloudflare.edge_cache_duplicate`): it turns on the Worker's edge cache
  and has no binding ([Edge caching](/book/reference/bynk-capabilities/#edge-caching-edgecache)).
  `Service`, `R2`, `RateLimit`, `Queue`, `Analytics` and `EdgeCache` are used only through instances
  (`bynk.capability.instance_only`).
- Only a context declares one (`bynk.capability.derived_outside_context`), and
  it cannot be exported (`bynk.exports.derived_capability`). The data belongs to
  the context, so another context reaches it through the context's services.

On Workers, each instance gets its own binding, named from the kind and the
capability (`Sessions` → `KV_SESSIONS`, `PriceCache` → `KV_PRICE_CACHE`), with an
`Env` member and a `[[kv_namespaces]]` stanza of its own. The bare `KV` stanza is
emitted only for a context that uses the bare `Kv`. Under `bynkc test`, each
instance has its own fresh in-memory double. `bynk deploy` provisions each
instance's namespace and records its id, titled `<worker>-<capability>`
([Several KV namespaces](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#several-kv-namespaces)). See [Use more than one KV namespace](/book/guides/effects-and-capabilities/use-several-kv-namespaces/).

A `Service` instance (`capability Billing = Service @worker("legacy-billing")`)
binds as `SERVICE_BILLING`, a `[[services]]` entry naming its Worker. Under
`bynkc test` its double faults until `send` is stubbed; `bynk deploy` binds it
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
  on call() -> Effect[Int] given platform.time.Clock {
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
`deps.<Cap>.op(…)`. See [emission](/docs/emission/).
