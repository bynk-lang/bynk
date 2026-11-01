---
title: Generics and bounds
---
A `fn` or a `type` may take **type parameters**, and a type parameter may carry
a **bound**: a constraint every type argument must meet. Bounds are checked at
compile time. An `Eq` bound is erased from the emitted TypeScript; an `Ord` bound
adds a hidden comparator parameter (see [Ord at run time](#ord-at-run-time)).
Code that uses no bound emits as before.

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
several. The constraints are **`Eq`**, which says values of the type compare with
`==` and `!=`, **`Ord`**, which says they have a total order (below), and
**`Key`**, which says they can key a `Map` or `Set` ([below](#key)), and
**`Codable`**, which says they cross the JSON boundary ([below](#codable)). Any other
name is `bynk.generics.unknown_constraint`.

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

## `Ord`

`T: Ord` says values of `T` have a total order, so a body may compare them with
`<` `<=` `>` `>=` and use them as a sort key. Which types are `Ord`, and the
order each one has, is under
[Ordering your own types](/book/reference/types/#ordering-your-own-types).

```bynk,fragment
commons demo.ord

uses bynk.list

fn top[T: Ord](xs: List[T]) -> Option[T] { max(xs) }

fn clamp[T: Ord](x: T, lo: T, hi: T) -> T {
  if x < lo { lo } else if x > hi { hi } else { x }
}

type Priority = enum { Low, Medium, High } derives Ord

fn highest(ps: List[Priority]) -> Option[Priority] { top(ps) }
```

- **`Ord` implies `Eq`.** A `T: Ord` may also use `==`. Writing `T: Eq + Ord` is
  the warning `bynk.generics.redundant_eq_bound`, and its fix removes `Eq`.
- **Ordering an unbounded `T`** (`<`, a `sortBy`/`min`/`max` key, or passing it on
  to an `Ord`-bounded parameter) is `bynk.generics.missing_ord_bound`. The fix is
  machine-applicable: it declares `: Ord`, replacing an existing `Eq`.
- **A call must instantiate an `Ord`-bounded parameter with an `Ord` type.**
  `top` over a `List[Bool]` or a record without `derives Ord` is
  `bynk.types.not_orderable`.
- **A bounded generic used as a value** (`xs.map(top)`) is instantiated at the
  reference, from the expected function type. If the type arguments cannot be
  resolved there, it is `bynk.generics.ambiguous_instantiation`; annotate the
  expected type.

```bynk,fail=bynk.generics.missing_ord_bound
commons demo.missing.ord

fn bigger[T](a: T, b: T) -> Bool { a > b }
```

`bynk.list` ships `sort`, `min` and `max` as `Ord`-bounded generics
([`bynk.list`](/book/reference/bynk-capabilities/#related-first-party-modules)).

### Ord at run time

An `Eq` bound adds nothing to the emitted code. An `Ord` bound does: the emitted
function takes one hidden trailing **comparator** parameter per `Ord`-bounded
type parameter, `(a, b) => -1 | 0 | 1`. A method takes the receiver type's
parameters first, then its own, each in declaration order. The caller builds the
comparator from the concrete type argument (or forwards its own, for a `T: Ord`
it is itself generic over). Where a comparison is on a base type, the emitted
code is still the bare operator.

These parameters are an implementation detail: they are not part of the
published ABI, since hand-written TypeScript bindings never call a Bynk generic.

## `Key`

`K: Key` says values of `K` can key a `Map` or `Set`. Which types are keys is under
[Keys](/book/reference/types/#keys). `Key` is automatic for concrete types and
declared for a type parameter, and it implies `Eq`: `K: Eq + Key` is the warning
`bynk.generics.redundant_eq_bound`. `Ord` does not imply `Key` (a `Float` is `Ord`
but never a key), so `T: Ord + Key` is a real pair.

A type parameter that reaches a `Map` key or a `Set` member, in a signature, a
generic type's field, or a `distinct`/`groupBy`/join key in a body, must be declared
`Key`, and a call must instantiate each `Key`-bounded parameter with a key type:

```bynk
commons demo.keybounds

fn tally[K: Key](xs: List[K]) -> Map[K, Int] {
  let init: Map[K, Int] = Map.empty()
  xs.fold(init, (acc, k) => {
    let seen = match acc.get(k) {
      Some(n) => n,
      None => 0,
    }
    acc.insert(k, seen + 1)
  })
}

type Index[K: Key, V] = { m: Map[K, V] }
```

```bynk,fail=bynk.generics.missing_key_bound
commons demo.missing.key

fn single[K](k: K) -> Map[K, Int] {
  let m: Map[K, Int] = Map.empty()
  m.insert(k, 1)
}
```

Without the bound `K` could be a function, so the error is
`bynk.generics.missing_key_bound`. The fix is machine-applicable, and
`bynk check --fix` applies it across a project, which is how code written before
`Key` (including a generic over `Map` or `Set`) is migrated. A method's receiver
parameter is bounded with `where K: Key`, or on the type.

`Key` costs nothing at run time and adds no hidden argument: composite keys are
held under a canonical encoding the runtime derives from the value itself.

## `Codable`

`Codable` says a value encodes and decodes with `Json`: `Json.encode(v)` and
`Json.decode[T](s)` work on a type parameter declared `T: Codable`.

```bynk
commons demo.codable

fn roundTrip[T: Codable](v: T) -> Result[T, JsonError] {
  Json.decode[T](Json.encode(v))
}

fn encodeAll[T: Codable](xs: List[T]) -> String { Json.encode(xs) }
```

`Codable` is automatic, never derived, and means exactly what `Json.decode` accepts:
base types (including `Float` and `Bytes`), refined and opaque types, `Option`,
`List`, `Set`, `Map` and `Result` of a `Codable`, records, sums and enums, and a
generic type instantiated at `Codable` arguments. It does not hold for top-level `()`
(it encodes but does not decode), functions, `Effect`, `Query`, a `Stream` or a type
that carries one, a held `Connection`, the HTTP and queue result types, `Asset`,
request headers, token claims or an actor. A call that instantiates a `Codable`
parameter with one of these is `bynk.types.not_codable`.

Without the bound, `Json.encode` or `Json.decode[T]` on a type parameter is
`bynk.generics.missing_codable_bound`, and so is passing the parameter to another
generic's `Codable` parameter; the diagnostic's fix adds the bound, and `bynk check
--fix` applies it across a project. Unlike `Ord` and `Key`, `Codable` does not imply
`Eq`, so the fix writes `T: Eq + Codable`. A method's receiver parameter is bounded
with `where A: Codable`.

**At run time**, a `Codable` parameter takes one hidden trailing argument, a codec
(`__codec_T`), built at the call site from the same generated `__serialise_X` and
`__deserialise_X` a monomorphic boundary uses. So a refined type is re-validated on
the way in, exactly as at an HTTP boundary. When a function has several witnesses they
follow the type parameters in declaration order, and within one parameter the
comparator comes first: `[T: Ord + Codable]` takes `__cmp_T` then `__codec_T`. Like an
`Ord` witness, a codec is internal and not part of the published ABI.

`List`, `Option`, `Set`, `Map` and `Result` over a type parameter encode and decode
inside the function; a `Set[T]` needs `T: Key + Codable`. A generic type of your own
over a type parameter (`Box[T]` inside `fn f[T: Codable]`) is not supported yet:
`bynk.generics.codec_generic_at_rigid`. A concrete `Box[Order]` is unaffected.

An opaque type decoded outside the unit that owns it is checked no more than it is at
any boundary there; `Codable` does not change that.

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

## Capability operations take `Key` and `Codable`

A generic capability operation (`fn dedup[T](key: String) -> …`) has no body for a
bound to serve, so `bynk.generics.bound_on_capability_op` rejects `Eq` and `Ord`.
`Key` alone is allowed (`fn snapshot[K: Key](keys: Set[K]) -> …`): it adds no hidden
argument, and without it an operation over a `Map[K, _]` could not be written.

`Codable` is allowed too (`fn append[T: Codable](id: String, event: T) -> Effect[()]`).
The **call site** encodes each argument whose parameter type mentions the `Codable`
parameter, whole, with the codec of its concrete type, and the emitted interface
declares that parameter as `unknown`: *a JSON value in Bynk's boundary encoding*. Other
parameters pass through unchanged (`id` stays a `string`). So a provider is
hand-written TypeScript that receives plain JSON, never a codec, and the operation's
ABI does not grow. The type arguments are inferred from the arguments when they
determine every parameter: `Queue.sendValue(order)`.

A `Codable` parameter may not appear in the operation's return type
(`bynk.generics.codable_op_return`): decoding a provider's reply raises what a failed
decode of stored data means, which is not settled. Typed reads wait for that. As for
any generic operation, it needs a provider written in TypeScript
(`bynk.provider.generic_op_requires_external`).

## Where this is going

`Eq`, `Ord`, `Key` and `Codable` are the constraints of the constrained-generics track.
`Eq` replaced the equality requirement the compiler used to infer, so the requirement
a caller must meet is declared in the callee's signature rather than discovered
from an error. Typed reads from a capability (a `Codable` return type) and a codec for a
generic type of your own over a type parameter are the open follow-ups.

## See also

- Guide: [Declare `Eq` on a generic that compares](/book/guides/type-system/declare-eq-bounds/).
- Guide: [Key a map by a record](/book/guides/type-system/key-a-map-by-a-record/).
- Guide: [Understand ordering your own types](/book/guides/type-system/ordering-your-own-types/).
- Guide: [Generic code that crosses the boundary](/book/guides/type-system/generic-code-that-crosses-the-boundary/).
- Troubleshooting: [`bynk.generics.missing_codable_bound`](/book/troubleshooting/generics-missing-codable-bound/),
  [`bynk.generics.missing_eq_bound`](/book/troubleshooting/generics-missing-eq-bound/),
  [`bynk.generics.missing_ord_bound`](/book/troubleshooting/generics-missing-ord-bound/).
- Spec: [§5.2 Generic instantiation](/book/spec/static-semantics/) and
  [§4.3.1b](/book/spec/syntactic-grammar/).
