---
title: Run independent calls at the same time
---
**Goal:** cut a handler's latency by issuing calls that do not depend on each other
together, instead of one after the other.

## Write a `par` block

Two effects that do not need each other's result cost the sum of their latencies when
each has its own `<-`:

```bynk
context shop.dashboard

capability Users {
	fn get(id: Int) -> Effect[String]
}

capability Orders {
	fn forUser(id: Int) -> Effect[Int]
}

provides Users = FixedUsers {
	fn get(id: Int) -> Effect[String] {
		"ada"
	}
}

provides Orders = FixedOrders {
	fn forUser(id: Int) -> Effect[Int] {
		id
	}
}

service summary {
	on call(id: Int) -> Effect[String] by Caller given Users, Orders {
		par {
			let user <- Users.get(id)
			let orders <- Orders.forUser(id)
		}
		Effect.pure("\(user): \(orders)")
	}
}
```

Both calls are issued, in the order written, before either is awaited. The handler
resumes when both have finished, and `user` and `orders` are in scope from the closing
brace on. A call that returns `Effect[()]` joins the block with `do`:

```bynk,fragment
par {
  let user <- Users.get(id)
  do Audit.touch(id)
}
```

## Keep the calls independent

The calls in a block run together, so none can use another's result. A binding that
mentions a name another binding of the block binds is refused
([`bynk.par.sibling_name`](/reference/diagnostics/)), even if an earlier `let` of
the same name is in scope. Without that rule the call would silently read the earlier,
stale value. If one call needs another's result, put it after the block:

```bynk,fragment
par {
  let user <- Users.get(id)
  let orders <- Orders.forUser(id)
}
let recent <- Orders.latest(orders)   // uses `orders`, so it comes after
```

A block holds only `let x <- e` and `do e`. Writes to an agent's `store` stay
sequential, so do them before or after the block, and a `?` is matched after the block
rather than inside it. The
[reference](/reference/capabilities/#par) lists everything that is refused.

The same holds for the closure you give to `parForEach`, `parTraverse`, `parTraverseAll` or
`parTraverseTry`: it runs for every element at once, so it may not write a `store`
(`bynk.par.store_write_in_closure`). A `get` followed by a `put` in it would lose
updates, because the branches interleave between the two calls. Run that loop with
`forEach`, where each element finishes before the next starts and a single-call
`update` stays atomic per key. A store write reads the entry before it stages the
write, so the sequential form reads one entry at a time; per-key atomicity is the
reason for the rule. To keep the concurrent part, collect with `parTraverseAll` and
write afterwards:

```bynk,fragment
// refused: the closure writes `counts`
let _ <- xs.parForEach((x: Int) => counts.update("k", (v) => v + 1))

// accepted: sequential, and `update` is atomic per key
let _ <- xs.forEach((x: Int) => counts.update("k", (v) => v + 1))
```

## Fan out over a list

To run one call per element at the same time, use `parForEach` when you only need the
calls to happen, and `parTraverse` when you want what they return. The results come back in
the order of the list, whatever order the calls finish in:

```bynk,fragment
fn quotes(skus: List[Sku]) -> Effect[List[Quote]] given Pricing {
	skus.parTraverse((s) => Pricing.quote(s))
}

fn warmAll(keys: List[String]) -> Effect[()] given Cache {
	keys.parForEach((k) => Cache.warm(k))
}
```

If a call faults, the others still finish, then the first fault in list order is raised, as for a
`par` block. `traverseAll` and `traverseTry` have concurrent forms (`parTraverseAll`, `parTraverseTry`) for
steps that return a `Result`.

## Test it

A `case` can check which names the block bound and the order its calls were issued in:

```bynk,fragment
case "both calls are issued, in order" {
  let s <- summary.call(1)
  expect Users.get before Orders.forUser
}
```

`before` compares the order the calls were *issued* in. It says nothing about which
finished first.

## See also

- [What a join guarantees on failure](/book/concepts/effects-and-capabilities/what-a-join-guarantees-on-failure/)
- [Reference: joining independent effects](/reference/capabilities/#par)
