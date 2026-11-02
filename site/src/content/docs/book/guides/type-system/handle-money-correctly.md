---
title: Handle money correctly
---
**Read this when** a value is a price, a tax, a balance or a rate, and `0.1 + 0.2`
must be `0.3`.

## Why not `Float`

A `Float` is an IEEE 754 double. It cannot hold `0.1` exactly, so sums drift
(`0.1 + 0.2 != 0.3`) and a JSON number loses precision once a client parses it as
a float. The older advice, to work in `Int` units (minor units, cents), is exact
but leaves every service to write its own rounding, formatting and wire shape,
and caps the magnitude at 2^53.

`Decimal` is exact: an arbitrary-precision coefficient with a scale. Write a
literal with a trailing `d`:

```bynk
commons guide.money

type Price = Decimal where Positive
type Rate = Decimal where InRange(0.00d, 1.00d)

fn lineTotal(unit: Price, qty: Int) -> Decimal {
  unit * Decimal.fromInt(qty)
}

fn withTax(net: Decimal, rate: Rate) -> Decimal {
  (net + net * rate).round(2, RoundingMode.HalfEven)
}
```

`+`, `-` and `*` are exact and never round, so `1.10d + 2.20d == 3.30d`. Rounding
happens only where you write it.

## Divide by stating the scale

`/` is not defined on `Decimal`: a quotient such as one third has no exact
decimal answer, and a language that picked a scale for you would pick it
silently. Division says how many digits to keep and how to round:

```bynk
commons guide.money.split

fn share(total: Decimal, ways: Int) -> Decimal {
  total.div(Decimal.fromInt(ways), 2, RoundingMode.Down)
}
```

`share(10.00d, 3)` is `3.33d`. Whatever rounding discards is not lost to you: the
remainder is `total - share * ways`, an exact `Decimal` you can allocate.

Pick the mode for the rule you are implementing: `HalfEven` (banker's rounding)
avoids a bias across many roundings, `HalfUp` is the schoolbook rule, `Down` never
overstates what a customer is charged. The seven modes and their effect on `2.5`
and `-2.5` are in the [reference](/book/reference/types/#decimal).

## Crossing a boundary

A `Decimal` field is a JSON **string** on the wire, so no client reads it as a
float:

```bynk
commons guide.money.wire

type Price = Decimal where Positive

type Line = { sku: String, unit: Price, qty: Int }

fn read(json: String) -> Option[Line] {
  match Json.decode[Line](json) {
    Ok(line) => Some(line)
    Err(_) => None
  }
}
```

`{"sku": "a", "unit": "4.50", "qty": 2}` decodes; `"unit": 4.5`, `"1e3"` and
`"-1.00"` do not (the last fails `Positive`). Encoding writes the canonical text
back, and keeps the scale: `4.50` stays `"4.50"`.

## Choosing `Int`, `Float` or `Decimal`

- **`Int`** for counts, identifiers and anything inherently whole. Minor units
  (cents) still work when the magnitude is small and you own the formatting.
- **`Float`** for measurements and anything approximate: sensor readings,
  scores, geometry. Compare with a tolerance.
- **`Decimal`** for money and any quantity where the digits you wrote are the
  digits you mean.

They never mix implicitly. Convert with `Decimal.fromInt(n)`, `d.toInt()` (only
for a whole number) and `d.toFloat()` (lossy).

A currency code is not part of the type. A `bynk.money` commons that pairs a
`Decimal` with an ISO 4217 code is a planned follow-on; until then, carry the
code in a record beside the amount.

**See also:** [Reference: Decimal](/book/reference/types/#decimal),
[Wire format](/book/reference/wire-format/),
[Spec §6.1.1](/book/spec/type-system/#611-base-types).
