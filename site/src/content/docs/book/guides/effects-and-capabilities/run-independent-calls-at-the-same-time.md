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
  fn get(id: Int) -> Effect[String] { "ada" }
}

provides Orders = FixedOrders {
  fn forUser(id: Int) -> Effect[Int] { id }
}

service summary {
  on call(id: Int) -> Effect[String] given Users, Orders {
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
([`bynk.par.sibling_name`](/book/reference/diagnostics/)), even if an earlier `let` of
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
[reference](/book/reference/capabilities/#par) lists everything that is refused.

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

- [What a join guarantees on failure](/book/guides/effects-and-capabilities/what-a-join-guarantees-on-failure/)
- [Reference: joining independent effects](/book/reference/capabilities/#par)
