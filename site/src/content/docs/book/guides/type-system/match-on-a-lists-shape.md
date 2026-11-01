---
title: "Match on a list's shape"
---
**Goal:** take a `List` apart by its length and its first elements, instead of
calling `length()`, `first()` and `get(i)` and checking each result.

A list pattern in a `match` does it, and the compiler checks that you covered
every length.

## Match the length

`[]` is the empty list. `[a, b]` is a list of exactly two elements, and a trailing
`..` stands for any further elements:

```bynk
commons shop {
  type Item = { sku: String, qty: Int }

  fn describe(items: List[Item]) -> String {
    match items {
      [] => "empty"
      [only] => only.sku
      [first, _, ..] => first.sku
    }
  }
}
```

The three arms cover every list: none, one, and two or more. No `_` arm is
needed, and leaving one out is a compile error that names what is missing:

```text
non-exhaustive `match` on `List[Item]` — `[_, _, ..]` is not covered
```

## Keep the rest

`..rest` binds the elements after the ones you named, as a `List`:

```bynk
commons shop {
  fn sum(xs: List[Int]) -> Int {
    match xs {
      [] => 0
      [x, ..rest] => x + sum(rest)
    }
  }
}
```

`[]` and `[x, ..rest]` together cover every list. `[..rest]` alone binds the
whole list.

> **Binding `..rest` copies.** `rest` is a new list of the remaining elements, so
> each match costs time in proportion to the list's length, and the recursive
> `sum` above is quadratic. Reach for `..rest` to peel off a few elements; to
> add up or transform every element, use a fold such as `xs.fold(0, (a, x) => a + x)`.
> A bare `..` (`[first, ..]`) copies nothing.

## Look inside the elements

Each element is any pattern, so literals, variants, records and further list
patterns work:

```bynk
commons shop {
  type Segment = { name: String }

  fn isApi(path: List[Segment]) -> Bool {
    path is [Segment { name: "api", .. }, ..]
  }

  fn firstValue(xs: List[Option[Int]]) -> Int {
    match xs {
      [Some(n), ..] => n
      [None, ..] => -1
      [] => 0
    }
  }
}
```

After `is`, a list pattern needs no parentheses: it ends at the `]`.

## Take a list apart with `let`

`let` with a list pattern needs an `else`, because the list may be shorter than
the pattern:

```bynk
commons shop {
  fn firstOrZero(jobs: List[Int]) -> Int {
    let [head, ..] = jobs else { 0 }
    head
  }
}
```

## What is not there

The `..` is always the last element. A pattern for the end of a list,
`[.., last]`, is not available, and writing one is
`bynk.parse.list_rest_not_last`. A `..` followed by a number (`[..9]`) is
reserved for range patterns and is `bynk.parse.range_pattern_unsupported`. Only a
`List` has list patterns: a `Set` or `Map` is not one
(`bynk.types.list_pattern_on_non_list`).

See [list patterns](/book/reference/types/#list-patterns) for the rules.
