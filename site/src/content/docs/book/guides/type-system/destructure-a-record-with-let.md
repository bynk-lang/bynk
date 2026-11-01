---
title: "Destructure a record with `let`"
---
**Goal:** bind several fields of a record at once, without a `let x = r.x` line
for each.

A record pattern in a `let` binds them in one.

## Bind fields

```bynk
commons shop {
  type Item = { sku: String, qty: Int, note: String }

  fn label(item: Item) -> String {
    let Item { sku, qty, .. } = item
    "\(sku) x\(qty)"
  }
}
```

`sku` and `qty` are in scope for the rest of the block. The trailing `..` is
required, as in every [record pattern](/book/reference/types/#record-patterns): it
says the other fields are ignored, so adding a field to `Item` never breaks the
`let`. No `else` is needed, because the pattern cannot fail to match an `Item`.

## Rename a field, ignore one, or go deeper

`f: name` binds the field under another name, `f: _` ignores it, and a field can
hold a nested record pattern:

```bynk
commons shop {
  type Customer = { name: String, email: String }

  type Order = { id: String, total: Int, customer: Customer }

  fn contact(o: Order) -> String {
    let Order { total: amount, id: _, customer: Customer { email, .. }, .. } = o
    "\(email): \(amount)"
  }
}
```

## Destructure an effect's result

`<-` works the same way:

```bynk,fragment
let Order { total, .. } <- Orders(id).current()
```

## When a field must match

A pattern with a literal or a variant for a field can fail, so it needs an
`else`, as any pattern `let` does:

```bynk
commons shop {
  type Status = enum { Pending, Paid }

  type Order = { id: String, status: Status, total: Int }

  fn paidTotal(o: Order) -> Int {
    let Order { status: Paid, total, .. } = o else { 0 }
    total
  }
}
```

Without the `else` the compiler says which field can fail
(`bynk.parse.let_pattern_without_else`). Going the other way, an `else` on a
pattern that cannot fail is `bynk.types.unreachable_arm`, and the editor offers a
fix that removes it.

A `let` pattern carries no type annotation, and the pattern may be wrapped in
parentheses (`let (Item { sku, .. }) = item`), though there is no need to.

## Destructure a lambda's parameter

A lambda parameter may be the same irrefutable pattern, which removes the
throwaway name and the repeated `item.`:

```bynk
commons shop {
  type Item = { sku: String, qty: Int, price: Int }

  fn skus(items: List[Item]) -> List[String] {
    items.map((Item { sku, .. }) => sku)
  }

  fn total(items: List[Item]) -> Int {
    items.fold(0, (sum, Item { qty, price, .. }) => sum + qty * price)
  }
}
```

Name parameters and pattern parameters mix freely (`sum` above), and the pattern
may nest. The parameter takes no annotation, because the type name already states
the type; for a generic record the type arguments come from where the lambda is
passed. A pattern that can fail (`status: Paid`) is
`bynk.parse.lambda_pattern_refutable`: take a name and `match` in the body
instead. Function and handler parameters stay plain names; start the body with a
`let` pattern to destructure one.

**See also:** [Match on a record's fields](/book/guides/type-system/match-on-a-records-fields/),
[Work with `Result` and optional values](/book/guides/type-system/result-and-optionals/#let-else),
[Reference — Record patterns](/book/reference/types/#record-patterns).
