---
title: Recursive generic types
---
**Do this when** you want a generic type that contains itself: a tree, a
non-empty list, an expression type. It declares and works inside a context like
any other generic.

```bynk
commons guide.tree

type Tree[A] =
  | Leaf
  | Node(left: Tree[A], value: A, right: Tree[A])

fn depth[A](t: Tree[A]) -> Int {
  match t {
    Leaf => 0
    Node(l, _, r) => {
      let dl = depth(l)
      let dr = depth(r)
      1 + if dl > dr { dl } else { dr }
    }
  }
}
```

A recursive edge must pass the type's own parameters through unchanged
(`Tree[A]` inside `Tree[A]`, also through `Option` or `List`). A type that
recurses at a *different* instantiation (`Weird[List[T]]` inside `Weird[T]`) is
refused at the declaration with `bynk.generics.polymorphic_recursion`.

## Where it cannot go yet

A recursive generic has no codec, so it cannot cross a boundary: a handler
signature, an agent `store` field or key, an event field, an actor identity
type, a `Json.encode`/`Json.decode` target or a `Codable` witness draw
`bynk.generics.recursive_generic_at_boundary`, once, at the use. A record that
contains a `Tree[Int]` is refused where *that record* is used, and the note
names the field it is reached through.

To cross a boundary, use a concrete recursive type
(`type IntTree = | Leaf | Node(left: IntTree, value: Int, right: IntTree)`),
which has a codec, or keep the generic tree inside the context and convert at the
edge.
