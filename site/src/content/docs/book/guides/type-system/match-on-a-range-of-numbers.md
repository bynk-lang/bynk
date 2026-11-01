---
title: "Match on a range of numbers"
---
**Goal:** pick a branch by which band an `Int` falls in (an age group, a status
code class, a score), instead of chaining `if n >= 13 && n <= 17` tests the
compiler cannot check.

A range pattern in a `match` does it, and the compiler checks that the bands
leave no gap and do not overlap by mistake.

## Write the bands

`lo..hi` matches every `Int` from `lo` to `hi`, **both ends included**. `lo..`
has no upper end and `..hi` no lower end:

```bynk
commons shop {
  fn bucket(age: Int) -> String {
    match age {
      ..-1 => "invalid"
      0..12 => "child"
      13..17 => "teen"
      18.. => "adult"
    }
  }
}
```

The four arms cover every `Int`, so no `_` arm is needed. Leave a band out and the
compiler names the gap:

```text
non-exhaustive `match` on `Int` — `13..17` is not covered
```

A gap of one value is named as that value (`100`), and an open gap as an open
range (`101..`). Paste the name as a new arm.

## Negative numbers

A bound may be negative: `-5..-1`, `..-1`. A bound is always a literal number;
digit separators work (`1_000..2_000`).

## Ranges inside other patterns

A range is a pattern wherever a number literal is, so it nests in variants,
records, lists, and or-patterns:

```bynk
commons shop {
  type Reading = { value: Int, unit: String }

  fn edge(reading: Option[Int]) -> String {
    match reading {
      Some(0..9 | 90..99) => "edge"
      Some(n) => "ok"
      None => "missing"
    }
  }

  fn warm(r: Reading) -> Bool {
    match r {
      Reading { value: 20..30, .. } => true
      _ => false
    }
  }

  fn small(xs: List[Int]) -> String {
    match xs {
      [] => "empty"
      [..9] => "one, at most nine"
      [_, ..] => "other"
    }
  }
}
```

In a list pattern, `..` followed by a number is a range, and `..` followed by a
name or nothing is the rest marker (`[first, ..rest]`).

## Overlap is allowed, repetition is not

A range that partly overlaps an earlier arm is fine; the earlier arm wins for the
shared values. A range that an earlier arm fully covers can never match, and is
reported:

```text
this match arm is unreachable — every value `3..5` matches is already matched by an earlier arm
```

## What is not there

- Only `Int` has ranges. A range against a `Float`, a `String` or anything else
  is `bynk.types.range_pattern_on_non_int`. `Float` ranges raise questions about
  `NaN` and representation that the language does not take on.
- `lo > hi` is `bynk.types.empty_range_pattern`, with a fix that swaps the bounds.
- `x is 13..17` is not allowed: `is` tests a type, not a value. Use a `match`
  arm, a guard (`n >= 13 && n <= 17`) or a refined type with `InRange`
  (`bynk.types.is_range_pattern`).
- A refined `Int` such as `Age = Int where InRange(0, 150)` is matched as an
  `Int`: its own bounds are not used to decide whether a `match` is complete.
- Exclusive bounds (`0..<10`) and naming the matched number (`n @ 0..9`) are not
  available; use a guard for the second.

See [range patterns](/book/reference/types/#range-patterns) for the rules.
