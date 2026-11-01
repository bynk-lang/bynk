---
title: Generics and bounds
---
A `fn` or a `type` may take **type parameters**, and a type parameter may carry
a **bound**: a constraint every type argument must meet. Bounds are checked at
compile time and erased in the emitted TypeScript, so a bounded generic emits
exactly what an unbounded one does.

## Type parameters

```bynk
commons demo.generics

type Box[A] = { value: A }

fn first[T](xs: List[T], fallback: T) -> T {
  match xs.get(0) {
    Some(x) => x
    None => fallback
  }
}
```

A function's type arguments are inferred from its arguments, or given explicitly
(`pick[Int](…)`). Inside the body a type parameter is **rigid**: equal only to
itself, so a generic function can move a `T` around but cannot look inside it.
Only a record or sum `type` may be generic.

## Bounds

A bound is written after the parameter name: `T: Eq`, or `T: C1 + C2` for
several. The only constraint is **`Eq`**, which says values of the type compare
with `==` and `!=`. Any other name is `bynk.generics.unknown_constraint`.

```bynk
commons demo.bounds

fn same[T: Eq](a: T, b: T) -> Bool { a == b }

fn dedupe[T: Eq](xs: List[T]) -> List[T] {
  xs
}
```

A type is **equality-supporting** unless it contains a function, `Effect`,
`Query`, `Stream` or held `Connection` anywhere inside it
([Operators & built-ins](/book/reference/operators/)). A bound makes that
requirement part of the signature, where hover, signature help and the reader
see it:

- `==` or `!=` on a value whose type mentions a type parameter requires that
  parameter to be declared `Eq`. Without the bound it is
  `bynk.generics.missing_eq_bound`, and the diagnostic's fix adds it.
- A call must instantiate each `Eq`-bounded parameter with an equality-supporting
  type: `same(f, h)` on two functions is rejected at the call
  (`bynk.types.not_comparable`).
- A generic that passes its own parameter to an `Eq`-bounded one must declare it
  `Eq` too, so a bound appears at every link of a chain of calls.

```bynk,fail=bynk.generics.missing_eq_bound
commons demo.missing

fn same[T](a: T, b: T) -> Bool { a == b }
```

An `Eq` bound costs nothing at run time: `==` on a `T` still lowers to the
runtime's structural comparison.

## A method's receiver parameters

A method on a generic type sees the type's parameters (`A` in `Box[A]`) without
declaring them. To compare one, bound it with a `where` clause after the return
type:

```bynk
commons demo.boxes

type Box[A] = { value: A }

fn Box.has(self, x: A) -> Bool where A: Eq { self.value == x }

fn Box.size(self) -> Int { 1 }
```

Only callers of `has` are constrained. A `Box[(Int) -> Int]` is still a valid
type, and `size` accepts it; `b.has(f)` on one is rejected at the call. A
`where` entry may name the method's own parameters too, and each entry takes
`+`-joined bounds like any other. Naming anything else is
`bynk.generics.unknown_where_param`.

## Bounds on type declarations

A `type` may bound its parameters as well, `type Pair[T: Eq] = { left: T, right:
T }`. The bound is checked where the type is applied to a written type argument:
`Pair[(Int) -> Int]` is rejected. A method of a bounded type sees the bound as
declared. No construct requires a bound on a type yet, so a bare type parameter
as the argument (`Pair[U]`) is not checked.

## Capability operations take no bounds

A generic capability operation (`fn dedup[T](key: String) -> …`) has no body for a
bound to serve. `bynk.generics.bound_on_capability_op` rejects one.

## Where this is going

`Eq` is the first constraint of the constrained-generics track. It replaced the
equality requirement the compiler used to infer, so the requirement a caller must
meet is declared in the callee's signature rather than discovered from an error.
Further constraints (ordering, map keys, boundary serialisation) will use the
same `T: C` syntax.

## See also

- Guide: [Declare `Eq` on a generic that compares](/book/guides/type-system/declare-eq-bounds/).
- Troubleshooting: [`bynk.generics.missing_eq_bound`](/book/troubleshooting/generics-missing-eq-bound/).
- Spec: [§5.2 Generic instantiation](/book/spec/static-semantics/) and
  [§4.3.1b](/book/spec/syntactic-grammar/).
