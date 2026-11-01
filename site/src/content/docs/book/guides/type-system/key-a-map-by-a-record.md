---
title: Key a map by a record
---
**Read this when** you want a `Map` or `Set` that looks things up by a record, an
enum, or a `Bytes`, and not just a `String` or an `Int`.

## Just use the type

A record, sum or enum whose fields are all keys is a key. There is nothing to
declare:

```bynk
commons guide.orders

type OrderId = { region: String, n: Int }

type Status = enum { Open, Shipped, Cancelled }

fn remember(m: Map[OrderId, Int], id: OrderId, qty: Int) -> Map[OrderId, Int] {
  m.insert(id, qty)
}

fn known(m: Map[OrderId, Int], id: OrderId) -> Bool {
  match m.get(id) {
    Some(_) => true,
    None => false,
  }
}

fn openOnly(xs: List[Status]) -> Set[Status] {
  Set.of(xs.filter((s) => s == Open))
}
```

Two `OrderId`s with the same fields are the same key, however they were built.
`m.keys()` gives back real `OrderId`s, in the order they were first inserted.

## What cannot be a key

A key must compare with `==` the way you expect, so a `Float` is out, at any depth
(`NaN` is not equal to itself), and so are `Map`, `Set` and functions. The error is
`bynk.types.not_keyable`, and it names the part that is not a key.

## A generic over keys declares `Key`

```bynk
commons guide.counting

fn countBy[K: Key, V](xs: List[V], key: (V) -> K) -> Map[K, Int] {
  let init: Map[K, Int] = Map.empty()
  xs.fold(init, (acc, x) => {
    let k = key(x)
    let seen = match acc.get(k) {
      Some(n) => n,
      None => 0,
    }
    acc.insert(k, seen + 1)
  })
}
```

Without `K: Key` the compiler reports `bynk.generics.missing_key_bound`; its fix
adds the bound, and `bynk check --fix` applies it across a project.

## Stored collections are different

A `store` map, set or cache still keys by `String` or `Int`. A value `Map` or `Set`
inside a stored record or a `Cell` may use any `Key` type.
