---
title: Use a literal where a refined type is expected
---
**Goal:** write a literal value directly where a refined type is expected,
without calling `.of` or handling a `Result`.

When you write a literal in a position whose expected type is a refined type,
Bynk checks the literal against the predicate **at compile time** and admits it
directly. A valid literal compiles; an invalid one is a compile error
([`bynk.refine.literal_violates`](/book/troubleshooting/refine-literal-violates/)).

## Where admission applies

```bynk
commons demo {
  type Quantity = Int where InRange(1, 100)

  // return position
  fn defaultQty() -> Quantity {
    5
  }

  // let with a type annotation
  fn sample() -> Quantity {
    let q: Quantity = 10
    q
  }

  // Ok / Some / Err payloads
  fn checked() -> Result[Quantity, ValidationError] {
    Ok(50)
  }

  // a refined-typed call argument
  fn clamp(q: Quantity) -> Quantity {
    q
  }
  fn useClamp() -> Quantity {
    clamp(10)
  }
}
```

Each admitted literal lowers to an inline brand cast (e.g. `(5 as Quantity)`) —
the check happened in the compiler, so none is needed at runtime.

## When to reach for `.of` instead

- The value is **not** a literal you write yourself (it comes from a request, a
  database, a variable): use [`.of`](/book/guides/type-system/define-and-validate/), which validates at
  runtime and returns a `Result`. A refined type has no unchecked escape hatch —
  a non-literal value always goes through `.of` (ADR 0182).
- **Opaque types are excluded** from literal admission — construct them with
  `.of`, or `.unsafe` within the opaque type's defining commons.

## Related

- Reference: [refined-type API](/book/reference/refined-types/).
- Rationale: [The refined-literal admission model](/book/guides/type-system/refined-literal-admission/)
  — including a [decision-flow diagram](/book/guides/type-system/refined-literal-admission/)
  for choosing between a literal and `.of`.
