---
title: Rename a field safely
---
A plain rename is the most common refactor and, for a field that is stored, locked or sent over a wire,
the most expensive: Bynk sees a field removed and a field added, so deployed data is not carried over and
callers built against the old name stop working. A `replaces` clause says it was a rename, and the
compiler reads the old name where the field is read.

This guide renames one field in each place it can live. The clause is the same in all three:
`name: T replaces old`.

## Rename a field in a record that crosses a wire

```bynk,fragment
type Order = {
	id: String,
	total: Int replaces amount,
}
```

Decoding now accepts both names: `{"amount": 5}` from a caller built against the old contract and
`{"total": 5}` from a new one both give `total` of `5`, and when both arrive the new key wins. Encoding
writes `total` only. Compile with the clause in place, deploy, and the callers that still send `amount`
keep working while you move them.

A request field is the case this fits. The contract diff in your editor reports a renamed request field
compatible. A renamed **response** field is not: the callee writes only the new key, so a caller built
against the old contract no longer finds it, and the diff reports it breaking. To rename a response field
without breaking callers, add the new field, keep the old one, and remove it once every caller has moved.

## Rename an event field

```bynk,fragment
event PaymentConfirmed = {
	orderId: String,
	total: Int replaces amount,
}
```

The registry reads the recorded `amount` as `total`, so the change is additive: the event's schema goes up
by one and `bynk.schema.lock` records `renamed_from = ["amount"]`. Events already written under `amount`
still decode. Commit the updated lock with the source change.

## Rename a stored field

```bynk,fragment
agent Cart {
	key id: String
	store owner: Cell[String] replaces customer = ""

	on call owner() -> Effect[String] {
		owner
	}
}
```

A record stored under `customer` loads as `owner`, and the first commit writes `owner` alone. The lock
records the rename and the change is additive, so no `migrate from` step is needed. This works for a `Cell`
field; a `Map`, `Set`, `Cache` or `Log` keeps its entries under keys that carry its name and is not renamed
this way (`bynk.store.replaces_collection`).

## Rename a field inside a stored record

A record that a `store` field holds is renamed the same way, in a `Cell` or in the value of a `Map`:

```bynk,fragment
type Prefs = { size: Int replaces width }

agent Cart {
	key id: String
	store prefs: Cell[Prefs]
	store named: Map[String, Prefs]
}
```

The lock reads the recorded `width` as `size`, so the change is additive and needs no `migrate from` step,
and a `Prefs` stored under `width` reads back as `size`. A rename that also changes the field's type is
still refused. The lock keeps no history for these renames, so deleting the clause is not refused while a
stored value still holds the old name: keep `replaces` until every value written before the rename has been
rewritten.

## When the clause can go

Keep `replaces` until nothing reads the old name:

- **A stored field**: while any stored version the lock holds keeps the value under the old name, deleting
  the clause is refused (`bynk.field.replaces_needed`). After `bynk migrate` raises the class's floor past
  those versions, the build warns (`bynk.field.replaces_settled`) that the clause can be deleted.
- **An event or wire field**: the compiler cannot see which payloads are still in flight, so it never says.
  Keep the clause until every producer built against the old name has been replaced and no event written
  under it can still be delivered.

Renaming again keeps the history: a field renamed twice still reads a record stored before the first
rename, as long as the lock holds one.

## What `replaces` refuses

- **A name that is not free** (`bynk.field.replaces_conflict`): the field itself, a field the record or
  agent still declares, or a name another field replaces.
- **A name the lock does not record** (`bynk.field.replaces_unknown`): a misspelling, or a field that was
  never deployed under that name.
- **A rename beside a retype or a removal**: still non-additive, as without the clause.

## See also

- [Renaming a store field](/reference/agents/#rename-a-store-field)
- [Renaming a field](/reference/types/#field-replaces)
- [The schema registry](/book/concepts/events/understand-events/#the-schema-registry-automatic-versions-verified)
