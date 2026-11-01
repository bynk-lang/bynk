---
title: "Design a library's public surface with `private`"
---
**Goal:** keep a commons' helpers to itself, so the units that `uses` it see
only the functions and types you mean them to depend on.

Every declaration is public by default: a unit that `uses` a commons can call
each of its functions and name each of its types. Mark a helper `private` and
it stays inside its own unit. Suppose a `pricing` commons computes a basket's
total, with a helper for a single line:

```bynk
commons pricing

type Item = { price: Int, qty: Int }

fn total(items: List[Item]) -> Int {
  items.fold(0, (acc, i) => acc + lineTotal(i))
}

private fn lineTotal(i: Item) -> Int {
  i.price * i.qty
}
```

`total` is the surface. `lineTotal` is how it works, and you can change or
remove it without breaking anyone.

## What `private` hides

A `private` declaration is visible throughout its own unit, across all of the
unit's files, and to the unit's suites. It is never visible through `uses`.

- **Functions and types.** `private fn` and `private type` work as above.
- **Methods.** `private fn Item.discounted(self)` hides one method while
  `Item` stays public. The methods of a private type are hidden with it.
- **Assets and embeds.** `private asset` and `private embed` hide the
  declaration's *name* from importers. A private asset is still served by the
  platform: `private` is about names, not about who can fetch the file.
- **Message bundles.** `private` on a bundle's `@reference` block keeps the
  bundle out of every context that uses the commons, so no context
  negotiates its locale against it; the commons' own `render` still reads it.
  That is rarely what you want, so the compiler warns
  (`bynk.visibility.private_messages`), as it does for `private` on any other
  block, where it has no effect.

A unit that `uses` `pricing` does not see `lineTotal` at all, so it may
declare its own `lineTotal` without a name conflict. If it calls `pricing`'s,
the compiler says why the call fails (`bynk.visibility.private_item`) rather
than reporting an unknown function:

```bynk,ignore
commons shop

uses pricing

fn cost(i: Item) -> Int {
  lineTotal(i)   // ✗ `lineTotal` is a private function of `pricing`
}
```

## Keep private types out of public signatures

A public function, method, record field or variant payload can't mention a
private type of its own unit, since an importer could neither name nor build a
value of it (`bynk.visibility.private_in_public`). Either make the type public,
or make the declaration that mentions it private too:

```bynk
commons pricing

type Item = { price: Int, qty: Int }

private type Tally = { lines: Int, sum: Int }

private fn tally(items: List[Item]) -> Tally {
  Tally { lines: items.length(), sum: items.fold(0, (acc, i) => acc + i.price * i.qty) }
}

fn summary(items: List[Item]) -> String {
  let t = tally(items)
  "\(t.lines) lines, \(t.sum) in total"
}
```

`summary` is public and returns a `String`, so `Tally` never escapes.

## Test the helpers

A suite targeting the unit sees its private declarations, so you can test a
helper directly:

```bynk,fragment
suite pricing {
  case "a line's total" {
    expect lineTotal(Item { price: 3, qty: 4 }) == 12
  }
}
```

The exception is for the suite's own target. A commons that the target uses
keeps its private declarations, in a suite as anywhere else.

A `system` suite goes further: it may name any type a participant context
declares, private ones included, though a context that consumes the
participant may not. A harness test often needs to build or inspect a value
the system keeps to itself.

## Where `private` doesn't apply

`private` modifies a `fn`, `type`, `messages`, `asset` or `embed` declaration
in a commons or a context. Services, agents, capabilities, providers, actors
and events can't be private (`bynk.visibility.private_not_allowed`): a
context's [`exports` and `consumes`](/book/guides/program-structure/consume-services/)
govern how far they reach.

`private` is a contextual keyword. Only immediately before one of those
declarations is it the modifier, so `@cache(scope: private)` and a parameter
named `private` mean what they always did.

## Related

- [Spec §5.1c — Unit-private declarations](/book/spec/static-semantics/#private-declarations)
- [Grammar reference — `visibility_modifier`](/book/reference/grammar/#rule-visibility_modifier)
- [How a Bynk program is shaped](/book/guides/program-structure/how-a-program-is-shaped/)
