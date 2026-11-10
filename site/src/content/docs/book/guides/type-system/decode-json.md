---
title: Decode untrusted JSON into a typed value
---
**Goal:** turn a raw JSON string from outside the program into a fully-typed —
and validated — Bynk value, with a typed error when it does not fit.

`Json.decode[T](s)` parses a JSON string and checks it against `T`, returning a
`Result[T, JsonError]`. The check covers both **shape** (the right fields and
base types) and any **refinements** on `T`, so untrusted JSON only ever enters
your program as a value that already satisfies its type's invariants — the
"make illegal states unrepresentable" idea, applied at the boundary.

## Decode into a record

```bynk
commons orders {
	type Order = {
		id: String,
		qty: Int,
	}

	fn parse(raw: String) -> Result[Order, JsonError] {
		Json.decode[Order](raw)
	}
}
```

The type argument is required (`Json.decode[Order](raw)`) — it tells the
compiler what shape to validate against. A missing field, a wrong base type, or
malformed JSON all produce an `Err(jsonError)`; a well-formed payload produces
`Ok(order)`.

An `Option` field is the exception to "a missing field is an error". JSON that
Bynk didn't write usually leaves an optional field out, or sets it to `null`, or
gives the bare value, and `decode` accepts all three:

```bynk
commons registry {
	type Repo = {
		repo: String,
		description: Option[String],
	}

	fn parse(raw: String) -> Result[Repo, JsonError] {
		Json.decode[Repo](raw)
	}
}
```

`{"repo": "a"}` and `{"repo": "a", "description": null}` decode with
`description` set to `None`, and `{"repo": "a", "description": "d"}` decodes
with it set to `Some("d")`. Bynk's own tagged form, which `Json.encode` writes,
also works: `{"kind": "Some", "value": "d"}` and `{"kind": "None"}`, as long as
the object has no other keys. A tagged object with an extra key, such as
`{"kind": "Some", "value": "d", "note": "x"}`, is read as a bare value instead.
A bare value is still checked as the inner type, so `"description": 7` is an
error, and so is that extra-key object.
See [the JSON codec](/book/reference/types/#the-json-codec) for the full rule.

## Decode a third-party payload with reserved-word keys

A payload from outside picks its own keys, and some are Bynk keywords. GitHub's
webhooks carry `type` on every account and `from` on a rename. A reserved word
can name a record field when it is written as a label, so the field is the key
and the record decodes as it stands:

```bynk
commons hooks {
	type Account = {
		login: String,
		type: String,
	}

	type Installation = {
		action: String,
		account: Account,
	}

	fn isOrganisation(raw: String) -> Bool {
		match Json.decode[Installation](raw) {
			Ok(i) => i.account.type == "Organization",
			Err(_) => false,
		}
	}
}
```

A match pattern names the field the same way, `Account { type: t, .. }`. The
shorthand `Account { type, .. }` would bind a variable called `type`, which a
keyword cannot be, so it is an error with a fix to `type: type_`. See
[reserved words as field names](/book/reference/types/#keyword-field-names).

## Decode straight into a refined type

Point `decode` at a type whose fields are refined, and the predicates are
enforced as part of decoding — no separate `.of` step:

```bynk
commons orders {
	type OrderId = String where NonEmpty

	type Quantity = Int where InRange(1, 1000)

	type Order = {
		id: OrderId,
		qty: Quantity,
	}

	fn parse(raw: String) -> Result[Order, JsonError] {
		Json.decode[Order](raw)
	}
}
```

`{"id": "", "qty": 5000}` is rejected — the empty `id` violates `NonEmpty` and
`5000` is outside `Quantity`'s range — so a decoded `Order` is guaranteed valid
everywhere downstream.

## Handle the result

`JsonError` is an ordinary error type, so handle it like any other `Result` —
propagate with `?`, or branch with `match`:

```bynk
commons orders {
	type Order = {
		id: String,
		qty: Int,
	}

	fn quantityOf(raw: String) -> Result[Int, JsonError] {
		let order = Json.decode[Order](raw)?
		Ok(order.qty)
	}
}
```

## Report why a decode failed

A `JsonError` carries three `String` fields: `kind` (`Malformed`,
`StructuralMismatch` or `RefinementViolation`), `path` (where in the document,
such as `$.qty`) and `message`. The error itself has no string form, so
interpolate the fields:

```bynk
commons orders {
	type Order = {
		id: String,
		qty: Int,
	}

	fn describe(raw: String) -> String {
		match Json.decode[Order](raw) {
			Ok(order) => "decoded \(order.id)",
			Err(e) => "order did not decode: \(e.kind) at \(e.path): \(e.message)",
		}
	}
}
```

Writing `"\(e)"` instead is refused, and the compiler's note points back at the
fields:

```bynk,fail
{{#include ../../../diagnostics/interpolate_json_error.bynk}}
```

```text
{{#include ../../../diagnostics/interpolate_json_error.txt}}
```

## Encode the other way

`Json.encode(v)` serialises a checked value to a JSON `String`:

```bynk
commons orders {
	type Order = {
		id: String,
		qty: Int,
	}

	fn render(o: Order) -> String {
		Json.encode(o)
	}
}
```

## Write JSON for a non-Bynk API

`Json.encode` writes an `Option` in Bynk's tagged form, which only Bynk reads back.
For a body a non-Bynk API receives, or a test stub that stands in for one, use
`Json.encodePlain`: `Some(v)` becomes `v` and `None` becomes `null`.

```bynk
commons github {
  type NewComment = { body: String, in_reply_to: Option[Int] }

  fn commentBody(c: NewComment) -> String {
    Json.encodePlain(c)   // {"body":"LGTM","in_reply_to":null}
  }
}
```

Pass the text as the `body` of a `Fetch.send` request. `Json.decode[NewComment]` reads
it back, since the decoder accepts both forms. See
[Plain JSON](/book/reference/types/#plain-json) for the two refusals.

**See also:** [Define a refined type and validate untrusted
input](/book/guides/type-system/define-and-validate/) (the `.of` path, for values you already have in
hand), and the [type-system reference](/book/reference/types/#the-json-codec).
