---
title: "`bynk.types.is_base_mismatch`"
---
```text
[bynk.types.is_base_mismatch] `is Quantity` checks an `Int` value, but got `String`
```

## What it means

`value is RefinedType` narrows a value to a refined type by checking its
predicates at runtime. That only makes sense when the value's type matches the
refined type's **base** — you cannot check a `String` against an `Int`-based
refinement.

```bynk,fail=bynk.types.is_base_mismatch
type Quantity = Int where InRange(1, 100)

fn f(s: String) -> Bool {
  s is Quantity        // Quantity is Int-based; s is a String
}
```

## Fix

Check the value against a refined type of its own base type, or convert first:

- For an `Int` value, use an `Int`-based refined type (`Quantity = Int where …`).
- For a `String` value, use a `String`-based one (`Code = String where …`).

## Related

- [Narrow and bind with `is`](/book/guides/type-system/narrow-with-is/)
- Reference: [Refined types](/book/reference/refined-types/)
