---
title: Decode a third-party webhook into typed values
---
A webhook from another service arrives in that service's shape: keys that are not identifiers
(`content-type`, `@odata.etag`), names you would not choose (`amount_captured`), and a variant told
apart by a key of its own (`"type": "charge.succeeded"`). A **wire name** maps each of those onto the
type you want to write in Bynk, in the type's declaration, so the payload decodes without any
hand-written parsing and the rest of your code never sees the external spelling.

This guide decodes a Stripe-shaped event and a GitHub-shaped one. The annotations are the same in
both: `@name("…")` on a field, a variant or a payload field, and `@discriminator("…")` on a sum.

## Map a field to its external key

Give a field the key the sender uses, and a better name in Bynk:

```bynk,fragment
type Charge = {
	id: String,
	@name("amount_captured") captured: Int,
	@name("created") at: Instant,
	@name("x-request-id") requestId: Option[String],
}
```

A body with `{"id": "ch_1", "amount_captured": 700, "created": 1700000000000}` decodes to a `Charge`
whose `captured` is `700`. In your code the field is `captured`: access, construction and
diagnostics use it. Only the decoder (and the encoder, OpenAPI and a decode error's path) use the
wire name, so a bad amount is reported at `$.amount_captured`, the key the sender wrote.

## Tell the variants apart by the sender's key

Stripe names the kind of event under `type` and its values with dots. Name the key once on the sum,
and each variant's value on the variant:

```bynk,fragment
@discriminator("type")
type StripeEvent =
	| @name("charge.succeeded") ChargeSucceeded(data: Charge)
	| @name("charge.refunded") ChargeRefunded(data: Charge)
	| Ping
```

`{"type": "charge.succeeded", "data": { … }}` decodes to `ChargeSucceeded`. You match on the Bynk
names as usual:

```bynk,fragment
fn describe(event: StripeEvent) -> String {
	match event {
		ChargeSucceeded(d) => "succeeded \(d.data.id)",
		ChargeRefunded(d) => "refunded \(d.data.id)",
		Ping => "ping",
	}
}
```

A variant without a `@name` keeps its own name (`Ping`). The default key `kind`, and the Bynk name
`ChargeSucceeded`, no longer read: the sum is read and written the one way.

## Map a payload field

GitHub names the payload of an event by the kind of its subject (`pull_request`, `issue`). A payload
field takes `@name` the same way:

```bynk,fragment
type PullRequest = {
	number: Int,
	title: String,
}

@discriminator("action")
type PullRequestEvent =
	| @name("opened") Opened(@name("pull_request") pr: PullRequest)
	| @name("closed") Closed(@name("pull_request") pr: PullRequest)
```

`{"action": "opened", "pull_request": {"number": 7, "title": "Fix"}}` decodes to
`Opened(pr)`, and `pr` is the `pull_request` value. A payload field may not take the discriminator's
name: `@name("action")` here would be `bynk.types.discriminator_collides`.

## Name an enum's values

A set of tags the sender spells in lower case works the same way, and is read as a bare string too:

```bynk,fragment
type Action = enum { @name("opened") Opened, @name("closed") Closed }
```

`"opened"` decodes to `Opened`. A query, path, header or cookie value of the type is the wire name.

Code that needs the text itself (a SQL parameter, a header, a log field) reads it instead of
writing a `match` that has to follow every variant and drifts from `@name`. `a.wireName()` is the
wire name of `a`, and `Action.fromWireName(text)` is its inverse, an `Option[Action]`:

```bynk,fragment
fn label(a: Action) -> String { a.wireName() }               // "opened"
fn parse(text: String) -> Option[Action] { Action.fromWireName(text) }
```

`fromWireName("opened")` is `Some(Opened)`; `fromWireName("Opened")`, the Bynk name, is `None`, because
the wire name is exact. Both read the one table the codecs read, so renaming a variant with `@name`
changes them together. They exist on an `enum`, not on a sum with a payload.

## What a wire name does not do

A wire name is part of the type's one encoding, so it applies everywhere the type is encoded: a
body, a result, `Json`, a call to another context, a queue, an event and stored state. That is also
why changing one is a change to the contract:

- On a type that only crosses an HTTP or `Json` boundary, a different name is a contract change,
  and the contract diff in your editor reports it as one.
- On a field of an event, or of a record stored in an agent, a different name changes what is on
  disk or in flight. Declare the move with `replaces`, which names the previous key, as in
  [Rename a field safely](/book/guides/type-system/rename-a-field-safely/).

See [Wire names](/reference/types/#field-name) for every rule and diagnostic.
