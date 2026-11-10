---
title: Constructors for generic types
---
**Read this when** a generic type must only be built one way, for example a
`Sorted[T]` that is sorted because `Sorted.fromList` is the only way in.

## Declare the constructor as a static

A static method on a generic type is generic over the type's parameters. A bound on
the type holds inside it:

```bynk,fragment
commons guide.sorted

uses bynk.list

type Sorted[T: Ord] = {
	items: List[T],
}

fn Sorted.fromList(xs: List[T]) -> Sorted[T] {
	Sorted { items: sort(xs) }
}

fn Sorted.empty() -> Sorted[T] {
	Sorted { items: [] }
}

fn ranked(names: List[String]) -> Sorted[String] {
	Sorted.fromList(names)
}
```

The caller never mentions `T`. `Sorted.fromList(names)` takes it from the argument, and
the compiler passes the comparator for `String`.

## When there is no argument

`Sorted.empty()` has nothing to infer `T` from. Give it a type to meet, or name it:

```bynk
commons guide.sortedempty

type Sorted[T: Ord] = {
	items: List[T],
}

fn Sorted.empty() -> Sorted[T] {
	Sorted { items: [] }
}

fn none() -> Sorted[Int] {
	Sorted.empty()
}

fn named() -> Int {
	let s = Sorted.empty[String]()
	0
}
```

`Sorted.empty[String]()` lists the type's parameters, then the method's own. Without
either, the call is `bynk.generics.uninferable_type_arg`.

## Why not a free function

`sortedFrom(xs)` works, but it lives beside the type under a name you invent, while
`Counter.zero()` on a plain type lives under the type. A static keeps one spelling for
both. A static that names none of its type's parameters (`fn Sorted.count() -> Int`)
cannot be inferred at all, so it is rejected; make it a free function.
