---
title: Check what a request stored
---
**Goal:** in a `system` case, assert on the rows a request wrote to a
participant's D1 database, when the app has no route that shows them.

A `system` case drives the whole app through its real boundaries, so it cannot
name a capability: `Orders.all(…)` there is `bynk.resolve.unknown_name`. To check
that "placing an order stored it", the case would have to go back out through
the app's own routes, which is awkward, or impossible when the app has none for
that. `D1.peek` gives the case a **read-only handle** on the participant's test
database instead.

## 1. The app

A context with a D1 database and a route that writes to it:

```bynk
context shop.orders

consumes bynk.cloudflare { D1, SqlValue }

capability Orders = D1

type Order = {
	id: String,
	owner: String,
	qty: Int,
}

service api from http {
	on POST("/orders/:id") (id: String) -> Effect[HttpResult[String]] by v: Visitor given Orders {
		let r <- Orders.run(
			"INSERT INTO orders (id, owner, qty) VALUES (?1, ?2, ?3)",
			[Text(id), Text("bob"), Integer(2)]
		)
		match r {
			Ok(_) => Ok("stored"),
			Err(_) => BadRequest("no"),
		}
	}
}
```

Its migration, `migrations/shop-orders-orders/0001_create.sql`, creates the table:

```sql
CREATE TABLE orders (id TEXT PRIMARY KEY, owner TEXT NOT NULL, qty INTEGER NOT NULL);
```

## 2. Read it from the case

Bind the handle with `let`, naming the participant and its D1 capability, then
read through it:

```bynk,fragment
suite shop.orders as system {
	case "placing an order stores it" {
		let db = D1.peek(shop.orders.Orders)
		let before <- db.select[Order]("FROM orders WHERE owner = ?1", [Text("bob")])
		expect before is Ok([])
		let r <- api.POST("/orders/:id", "o1") by Visitor
		expect r is Ok("stored")
		let after <- db.select[Order]("FROM orders WHERE owner = ?1", [Text("bob")])
		expect after is Ok([Order { id: "o1", owner: "bob", qty: 2, .. }])
	}
}
```

The handle offers the reads, with the capability's own signatures: `all`,
`first`, and the typed `select[T]` and `firstAs[T]`. Its SQL is literal and is
checked against the migrations' schema like any other D1 call, so a misspelt
column is a diagnostic in the test, not a failure at run time. The `D1` value
types (`SqlValue` and the rest) are in scope in the case with no `consumes`.

## 3. What it reads

The handle reads the **same** in-memory database the app's own D1 calls run
against in this case, with the participant's migrations applied. It sees exactly
what the app wrote, as it wrote it, and each case starts from an empty database.
A handle may be named anything and copied (`let other = db`), but it is bound by
`let`: a `D1.peek` anywhere else is `bynk.test.peek_not_bound`.

## 4. It only reads

A statement that would write is refused, as is `run` or `batch` on the handle
(`bynk.test.peek_write`). The leading keyword (`INSERT`, `UPDATE`, `DELETE`, a
schema change) is enough to refuse one at check time, with or without a schema;
with the schema, a write the keyword hides, such as `WITH … DELETE`, is found
from the prepared statement; and the test database refuses a write again at
run time. A peek that could write would let a case put the database in a state
the app's own invariants never saw, so to get state in, drive the app.

Starting a case from a known row without driving the app is tracked in
[#3563](https://github.com/bynk-lang/bynk/issues/3563).

## 5. Where it runs

`D1.peek` is valid in an `integration` or `system` case, on a capability
derived from `D1` that the participant declares. Elsewhere it is refused, each
with its own diagnostic: `bynk.test.peek_outside_case`,
`bynk.test.peek_not_a_database`, `bynk.test.peek_unknown_capability`.

Under `bynk test --against`, a case that uses it is refused
(`bynk.test.remote_unsupported_peek`), like the other cases a real runtime can't
honour, or skipped, saying why, under `--skip-unsupported`: a deployment has no
in-process database, and reaching a deployed one would make a test run touch
real data. See
[Testing against a running system](/book/guides/testing/against-a-running-system/).

**See also:** [Reference — Reading a participant's database](/reference/testing/#peek-a-database).
