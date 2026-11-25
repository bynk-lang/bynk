---
title: Delete a user's data
---
**Goal:** remove everything an agent stores for one key, such as a closed account's data
or a finished order's, so the key reads as one that was never used.

## Erase the instance

`self.erase()` in an agent handler deletes the instance's stored state when the handler
commits:

```bynk,fragment
agent Account {
	key id: String
	store email: Cell[Option[String]]
	store orders: Map[String, Order]

	on call close() -> Effect[()] given Events {
		do Events.emit[AccountClosed](AccountClosed { id: self.id })
		self.erase()
	}
}
```

Every field, every `Map`, `Set`, `Cache` and `Log` entry, every `@indexed` posting and the
schema markers for this key go. The next call starts from the agent's initial state, exactly
as a key never seen does. Clearing a `Map` entry by entry, or setting a `Cell` back to its
zero, leaves the instance's record and anything the handler does not know to remove.

Call it with `do self.erase()` in the middle of a body, or as the handler's last expression.

## Say that it happened first

The erasure is the handler's commit, so the handler's other writes are discarded with the rest.
To keep a record of the erasure, emit an event before it: events emitted in the handler are
still released, so a subscriber, or an audit log, learns that the account closed.

A handler that faults commits nothing. One that calls `self.erase()` and then faults erases
nothing, and the instance is as it was.

## Test it

A case observes the reset by calling a read handler, as any user of the agent would:

```bynk,fragment
case "close deletes the account's data" {
	let _ <- Account("a").open("a@b.c")
	let before <- Account("a").orderCount()
	let _ <- Account("a").close()
	let after <- Account("a").orderCount()
	expect before == 2
	expect after == 0
}
```

At `unit` and `integration` the instance's in-memory storage is deleted. At `system` the
Durable Object's storage is.

**See also:** [Reference — Erasing an instance](/reference/agents/#erase),
[Reference — Renaming and removing an agent](/reference/agents/#renaming-and-removing-an-agent),
which removes every instance of a class.
