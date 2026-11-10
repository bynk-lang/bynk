---
title: Define and consume sum, record, and opaque types
---
**Goal:** declare each of Bynk's three composite type kinds and use their values.

## Record — group fields

```bynk
type Order = {
  id: String,
  item: String,
}
```

Construct by naming every field; read with dot access; produce a changed copy
with the spread form:

```bynk,fragment
fn rename(o: Order, item: String) -> Order {
  Order { ...o, item: item }
}
```

Records are immutable — the spread copies and overrides.

## Sum — one of several variants

A variant may carry a payload or not:

```bynk
type Status =
  | Pending
  | Shipped(tracking: String)
  | Cancelled(reason: String)
```

Construct by naming a variant (`Pending`, `Shipped("1Z…")`); consume with
[`match`](/book/guides/type-system/match/), which must cover every variant.

## Opaque — a distinct identity

An opaque type is backed by another type but is not interchangeable with it:

```bynk
type OrderId = opaque String
```

Inside the module that defines it, construct with `OrderId.unsafe("ord-1")` (or
`OrderId.of(...)` for a checked `Result`). You cannot pass a plain `String` where
an `OrderId` is expected, which is the point.

## Putting them together

```bynk
commons shop {
	type OrderId = opaque String

	type Status =
		| Pending
		| Shipped(tracking: String)

	type Order = {
		id: OrderId,
		status: Status,
	}

	fn newOrder(id: OrderId) -> Order {
		Order {
			id: id,
			status: Pending,
		}
	}
}
```

## Related

- Tutorial: [Model your data with types](/book/tutorials/03-modelling-data/).
- Reference: [type system](/book/reference/types/).
- Rationale: [The type-system philosophy](/book/guides/type-system/philosophy/).
