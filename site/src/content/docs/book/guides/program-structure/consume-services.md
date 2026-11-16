---
title: "Consume another context's services with `consumes`"
---
**Goal:** call a service that belongs to another context.

A context declares the contexts it depends on with `consumes`, then calls their
services. Suppose a `payment` context offers an `authorise` service:

```bynk
context payment

service authorise {
	on call(amount: Int) -> Effect[Result[Int, Int]] {
		Ok(amount)
	}
}
```

## Declare the dependency and call the service

In the consuming context, declare `consumes <context>` and call the service by
its qualified name. Service calls are effectful, so bind the result with `<-`:

```bynk,fragment
context orders

consumes payment

service placeOrder {
	on call(total: Int) -> Effect[Result[Int, Int]] {
		let _ <- payment.authorise(total)
		Ok(total)
	}
}
```

## Use an alias

Add `as <Alias>` to call through a shorter name:

```bynk,fragment
context orders

consumes payment as Pay

service placeOrder {
	on call(total: Int) -> Effect[Result[Int, Int]] {
		let _ <- Pay.authorise(total)
		Ok(total)
	}
}
```

## Consume in every file that names it

A context can be split over several files that share its declarations, but
`consumes` is not shared: **each file consumes what it names.** A name from
another context or from the standard library is in scope in a file only if that
file has a `consumes` for it, and where the list is braced, only if it is
listed. Two files can consume the same thing; the context depends on the union
of what its files consume.

```bynk,fragment
context orders

consumes bynk { Clock }
consumes payment

service placeOrder {
	on call(total: Int) -> Effect[Result[Int, Int]] given Clock {
		let _ <- payment.authorise(total)
		Ok(total)
	}
}
```

A second file of `orders` that also calls `payment.authorise`, or writes
`given Clock`, needs its own `consumes payment` and `consumes bynk { Clock }`.
A file that does not is an error
(`bynk.consumes.unit_not_consumed`, `bynk.consumes.capability_not_selected`,
`bynk.consumes.type_not_selected`), and one command adds the clauses to every
file that needs them:

```sh
bynk check --fix=bynk.consumes.type_not_selected,bynk.consumes.capability_not_selected,bynk.consumes.unit_not_consumed
```

A suite file consumes what it names in the same way: see
[what a suite consumes](/book/reference/testing/#suite-consumes).

## Rename an exported type that clashes with one of yours

`consumes U { … }` brings into scope the names it lists, and the types its
listed capabilities' signatures use. `bynk.crypto` exports `Digest`,
`Signature`, `CryptoError`, `SigningKey` and `VerifyKey`, and `Crypto`'s
operations use all of them, so a context that also has its own `Digest` (from a
`uses` commons, or declared locally) is `bynk.consumes.name_conflict`.

A type the listed capabilities do *not* use is out of scope: `consumes bynk
{ Clock }` does not bring in `Method`, so a context with its own `Method` has no
clash, and the standard library can add a type without changing what your
program means. Naming a standard-library type the braces do not list is the error
`bynk.consumes.type_not_selected` (`bynk check --fix` adds the name). The same goes for a
file whose own `consumes` does not bring the type in, as above. A bare `consumes
bynk`, with no braces, brings in the whole module for the file that writes it.

Rename the consumed type in the braces with `as`.
It is then in scope only under the alias, and the clash goes away:

```bynk,fragment
context app

uses report
consumes bynk.crypto { Crypto, Digest as CryptoDigest }

fn empty() -> Digest {
	Digest { total: 0 }
}

service hashing {
	on call(token: String) -> Effect[Bytes] given Crypto {
		Crypto.digest(CryptoDigest.Sha256, Bytes.fromUtf8(token))
	}
}
```

The alias is the same type under another name, not a new one: it works wherever
the original would, as a type, a constructor, a variant (`CryptoDigest.Sha256`),
a pattern, or an argument to the capability's operations. The original name,
`Digest` here, is not in scope from `bynk.crypto`, so it means your own type.

The same works for a type another context exports
(`consumes lib { Code as LibCode }`): the alias is a name in this context only,
and values cross the service boundary as `lib`'s own type. A capability keeps
its name (`bynk.consumes.type_alias_capability`); alias a name the unit does not export
(`bynk.consumes.type_alias_unknown_export`), alias the same type twice or both
bare and aliased (`bynk.consumes.type_alias_repeated`), or pick an alias that is
already taken (`bynk.consumes.type_alias_conflict`), and the compiler refuses.
In a context split across files, an alias written in any file applies to all of
them.

An alias that appears in one of this context's own service signatures is part of
that service's contract: renaming the alias changes the contract hash of every
service whose parameters or result name it, so the callers of those services
must be rebuilt and redeployed with it (a stale caller is refused with
`ContractMismatch`, as for any other signature change).

## Undo earlier steps when a later one fails

Each context commits its own work. When a handler calls several, nothing rolls
the earlier calls back if a later one fails, so the handler undoes them itself.
`let <pattern> <- <call> else { … }` puts each step's undo beside the step
([`let … else`](/book/guides/type-system/result-and-optionals/#let-else)):

```bynk,fragment
context orders

consumes inventory as Stock
consumes payment as Pay

service placeOrder {
	on call(sku: String, total: Int) -> Effect[Result[Int, String]] {
		let Ok(_) <- Stock.reserve(sku) else { Err("out of stock") }
		let Ok(receipt) <- Pay.authorise(total) else {
			let _ <- Stock.release(sku)
			Err("payment declined")
		}
		Ok(receipt)
	}
}
```

If `reserve` fails, nothing has happened yet, so the handler answers at once. If
`authorise` fails, it releases the stock it reserved first. Either `else`
produces the handler's result, in place of the rest of its body.

## How it compiles

The call is the same Bynk code on both targets, but the emitted wiring differs:

- **`bundle`** (default) — a direct in-process function call through a composed
  surface.
- **`workers`** — a JSON call over a Cloudflare Service Binding, with the
  arguments and result validated as they cross the boundary.

See [Target Cloudflare Workers](/book/guides/projects-build-and-deployment/cloudflare-workers/) for the
target details.

## Related

- Reference: [type system](/book/reference/types/).
- Explanation: [How a Bynk program is shaped](/book/guides/program-structure/how-a-program-is-shaped/).
