---
title: Write a library that uses a capability
---
**Goal:** write a library function once, in a commons, that reads and writes a KV namespace,
and let each context that calls it say which of its namespaces the call uses.

A commons has no capabilities of its own: it never owns data. A function in one can still
*use* a capability, as a **capability parameter** that every caller binds to one of its own
instances. Capability parameters are **experimental**: they carry no compatibility promise
yet. Opt in in `bynk.toml`:

```toml
[unstable]
allow = ["bynk.given.parameters"]
```

## Declare the parameter

Select the kind in the commons, as you would a type, then name a parameter bounded by it in
the function's `given`:

```bynk,fragment
commons lib.cache

consumes bynk { Logger }
consumes bynk.cloudflare { Kv }

fn remember(key: String, value: String) -> Effect[String] given Store: Kv, Logger {
	let hit <- Store.get(key)
	match hit {
		Some(v) => Effect.pure(v),
		None => {
			let _ <- Logger.info("cache miss: \(key)")
			let _ <- Store.put(key, value)
			Effect.pure(value)
		},
	}
}
```

`Store: Kv` reads "a `Kv` the caller chooses, called `Store` here". `Kv` has instances, so a
commons always names it as a parameter: a bare `given Kv` is `bynk.given.kind_needs_parameter`.
`Logger` has none (a context has one log), so it stays a **shared** capability, named bare and
supplied by the caller's own `given`.

## Bind it at every call

A context that `uses` the commons binds the parameter inside the call's arguments, after
`given`, to a capability it holds:

```bynk,fragment
context shop

uses lib.cache
consumes bynk { Logger }
consumes bynk.cloudflare { Kv }

capability Sessions = Kv

capability Carts = Kv

service api {
	on call(id: String) -> Effect[String] by Caller given Sessions, Logger {
		remember(id, "fresh", given Store = Sessions)
	}
}
```

The binding is written at every call, even when the caller holds only one `Kv`: reading the
wrong namespace would type-check, and nothing else would catch it. Leave it out and the call is
`bynk.given.binding_missing`. Its fix offers one binding per instance you hold, and `bynk check
--fix` applies it only when there is exactly one, so it never chooses between `Sessions` and
`Carts` for you.

## Forward it through a chain

A library is usually several small functions. One that calls another binds the callee's
parameter to its own, `given Inner = Store`: the left name is the callee's, the right the
caller's.

```bynk,fragment
fn evict(key: String) -> Effect[()] given Inner: Kv {
	Inner.delete(key)
}

fn evictAll(keys: List[String]) -> Effect[()] given Store: Kv {
	keys.forEach((k) => evict(k, given Inner = Store))
}
```

A function with capability parameters is not a value, because its bindings would be left
unchosen: write the lambda that binds instead, as `evictAll` does
(`bynk.given.parameterised_fn_value` otherwise).

## Test it

In the commons' own suite, bind the parameter to the kind's in-memory double, named by the
kind, then stub and observe it. A function that takes two parameters of one kind needs a second
double: declare a suite-local instance.

```bynk,fragment
suite lib.cache

consumes bynk { Logger }
consumes bynk.cloudflare { Kv }

capability Archive = Kv

case "a miss writes the value" {
	stub Kv.get(_) returns None
	let v <- remember("k", "v", given Store = Kv)
	expect v == "v"
	expect Kv.put called once
	expect Logger.info called
}
```

In a context's suite, a call through `Store` reaches the instance the context bound, so the case
stubs and observes that instance: `stub Sessions.get(_) returns None`, `expect Sessions.put
called once`. A test never names a parameter: `stub Store.get` is `bynk.stub.not_a_seam`, with a
note naming what to stub instead.

## In the editor

Hover on a call shows the function's parameters with their kinds (`given Store: Kv, Logger`)
and the instance each one is bound to at that call. Completion after `given Store = ` offers only
your instances of `Kv`. Signature help shows the parameters as you type the call.

**See also:** [Capability parameters in a commons](/reference/capabilities/#capability-parameters),
[Use more than one KV namespace](/book/guides/effects-and-capabilities/use-several-kv-namespaces/),
[Share a handler step that logs or queries](/book/guides/effects-and-capabilities/share-a-handler-step/).
