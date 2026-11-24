# 0935 — filterMap, zipWith, takeWhile, skipWhile and count(p) on List

- **Status:** Accepted (v0.677)

**Context.** Keep-and-transform, pairwise combination, a prefix by predicate and counting matches each took two passes or a hand-written `fold` with a flag. There are no tuples, so `zip` has no result type; `zipWith` is its tuple-free form.

**Decision.** Five `List` methods. `filterMap(f: T -> Option[U])` keeps the `Some` results in order. `zipWith(other: List[U], f: (T, U) -> V)` stops at the shorter list (an error or `Result` on unequal lengths was rejected: a caller who needs equal lengths checks `length()`). `takeWhile` and `skipWhile` split at the first element failing `p`. `count(p)` sits beside `count()`: arity distinguishes them, as with `min`/`max`, and it counts without building the filtered list. They are plain `ListOp`s checked in a new module (`list_more.rs`) and lowered in `lower_list/more.rs`; there is no runtime change. `Query` parity (Decision C: `filterMap`, `takeWhile`, `skipWhile`, `count(p)`) is not included: a `Query` builder is lowered through the runtime query builder, so each of these needs runtime query support, not a plain map onto the existing pipeline. That is deferred.

**Consequences.** Additive: no existing program changes. The `count` signature in hover and signature help shows both arities.
