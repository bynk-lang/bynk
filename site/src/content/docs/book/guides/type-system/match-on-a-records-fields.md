---
title: "Match on a record's fields"
---
**Goal:** branch on, and bind, the fields of a record, without nesting `if`s or
matching one field at a time.

A variant payload pattern stops at the first record it meets: `Some(order)` binds
the whole record. A **record pattern** looks inside it.

## Match fields in a `match`

Name the record type, then list the fields you care about and end with `..`:

```bynk
commons shop {
  type Status = enum { Pending, Paid, Cancelled }

  type Order = { id: String, status: Status, total: Int }

  fn describe(o: Order) -> String {
    match o {
      Order { status: Paid, total, .. } if total > 100 => "large paid order"
      Order { status: Paid, .. } => "paid"
      _ => "other"
    }
  }
}
```

- `total` binds the field to a local named `total`.
- `status: Paid` matches the field against another pattern: a variant, a
  literal, or a nested record pattern.
- A guard (`if total > 100`) sees the names the pattern bound.

## Always end with `..`

Every record pattern ends with `..`, even one that lists every field. It says
"and the rest" at the place you read it, and it is why **adding a field to the
record never breaks a pattern**: the patterns above compile unchanged when `Order`
gains a `note` field. If you leave it out, the editor offers to insert it
(`bynk.parse.record_pattern_without_rest`).

## Look inside an `Option` or a `Result`

Record patterns nest inside variant payloads:

```bynk
commons shop {
  type Status = enum { Pending, Paid, Cancelled }

  type Order = { id: String, status: Status, total: Int }

  fn paidTotal(o: Option[Order]) -> Int {
    match o {
      Some(Order { status: Paid, total, .. }) => total
      _ => 0
    }
  }
}
```

## Test a record with `is`

After `is`, put the record pattern in parentheses. Without them the `{` would
also open the `if` block:

```bynk
commons shop {
  type Status = enum { Pending, Paid, Cancelled }

  type Order = { id: String, status: Status, total: Int }

  fn billed(o: Order) -> Int {
    if o is (Order { status: Paid, total, .. }) { total } else { 0 }
  }
}
```

The names the pattern binds are in scope in the `if`'s then-branch, as for a
variant payload.

## Cover the rest

Arms cover a record between them. If every `status` has an arm, no `_` is needed:

```bynk,fragment
match order {
  Order { status: Paid, .. } => 1
  Order { status: Pending, .. } => 2
  Order { status: Cancelled, .. } => 3
}
```

Leave one out and the compiler names the pattern to add, such as
`Order { status: Cancelled, .. }`; the editor's *add missing arms* fix inserts
it. A later arm that the earlier ones already cover, even all together, is
`bynk.types.unreachable_arm`.

To bind a record's fields without a `match`, see
[Destructure a record with `let`](/book/guides/type-system/destructure-a-record-with-let/).

**See also:** [Pattern-match with `match`](/book/guides/type-system/match/),
[Narrow and bind with `is`](/book/guides/type-system/narrow-with-is/),
[Reference — Record patterns](/book/reference/types/#record-patterns).
