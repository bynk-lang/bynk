# 0681 — Ordering is declared with `derives Ord` and bounded with `T: Ord`, and a bounded generic takes comparator witnesses

- **Status:** Accepted (v0.465)

**Context.** Orderability was a fixed list of five base types, in the `List`/`Query`
`sortBy`/`min`/`max` key rule and again in the comparison operators. A generic
body could do nothing with a `T` that needs an order, and a user type could be
sorted only through a key function. An order needs a definition per type (variant
order, field order, how `Float`'s `NaN` sorts), and under erasure the runtime
cannot recover it: a nullary variant is a bare `{ tag }` with no position and
object key order is not declaration order. Only the compiler knows the
declaration. The constrained-generics track (`design/tracks/constrained-generics.md`)
settled the model: a closed constraint set (Q1), `Ord` declared and not
structural (Q2 (c)), witness passing for bounded parameters (Q4 (b)) and the
meaning of `Ord` (Q5). This is slice 1 (#2660), which builds on slice 0's bounds
and constraint table (ADR "declared-eq-bounds"). The maintainer approved #2660
as written on 2026-11-01, the Recommended option for every decision below.

**Decision.**
1. **`derives` syntax (A).** A trailing `derives Ord` clause after a type's body.
   `derives` is a contextual word, an ordinary identifier everywhere else, read
   only after a record, sum or enum body, or after an opaque type's or refined
   type's base and refinement. The list form `derives Ord + …` parses so later
   constraints add no grammar. `derives Eq`, `Key` and `Codable` are
   `bynk.types.derives_automatic` with a machine-applicable fix that removes the
   name; any other name is `bynk.generics.unknown_constraint`. A `derives Ord` on
   a refined type is `derives_automatic` too, since a refined type over an
   orderable base is `Ord` without it.
2. **What is `Ord` (B).** `Int`, `Float`, `String`, `Duration`, `Instant`, and
   refined types over them; `Bytes`, unsigned lexicographic, which flips
   `negative/2016_bytes_sortby_key` and `2016_bytes_min_key` to positive;
   `Option[T]` and `List[T]` when `T` is `Ord`; a record, sum, enum or opaque type
   that `derives Ord` (a generic one is `Ord` at an instantiation whose arguments
   are); a type parameter declared `T: Ord`. `Bool`, `Result`, `Map`, `Set`,
   functions and held resources are not. `derives Ord` requires every field and
   payload to be `Ord`, the declaration's own parameters taken as `Ord`
   (`bynk.types.derives_field_not_orderable`, naming the field). A recursive reference that changes the type's arguments (`Nest[List[T]]` inside `Nest[T]`) is rejected the same way, since its comparator would need one nesting level per depth. Anything else
   that is ordered is `bynk.types.not_orderable`. An opaque type is not `Ord`
   outside its defining commons unless it declares `derives Ord` there (Q2). A
   record orders by its fields in declaration order, a sum by variant declaration
   order and then payload, `Option` puts `None` first, `List` is lexicographic.
   `String` order is normatively UTF-16 code-unit order, which is what it was.
3. **`Ord` implies `Eq` (C).** A `T: Ord` parameter may use `==`; `has_eq` accepts
   `Ord`. `T: Eq + Ord` is the warning `bynk.generics.redundant_eq_bound`, with a
   machine-applicable fix that removes `Eq`.
4. **Witnesses (D).** An `Ord`-bounded type parameter takes one hidden trailing
   comparator parameter `(a, b) => number` (negative, zero or positive), in the
   order: the receiver type's parameters first, in the type's declaration order,
   then the function's or method's own, in declaration order. A parameter is
   bounded by `T: Ord`, by a `where A: Ord` entry, or, for a receiver parameter,
   by the type's own bound. A call site builds the witness from the concrete type
   argument, or forwards the caller's own for a rigid `T: Ord`. The checker
   resolves the type arguments and records them on the call's `Callee`; the
   emitter builds the expressions. Witnesses are internal and not added to the ADR
   0310 ABI, which is safe because bindings never call Bynk generics: bindings
   implement emitted capability interfaces, so calls run from Bynk into TypeScript
   only, and the one way a type parameter reaches a binding, a generic capability
   operation, takes no bounds (`bynk.generics.bound_on_capability_op`). An
   `Eq`-only bound adds nothing; unbounded and non-`Ord` code emits byte-identically.
   *Refinement of the proposal's shape:* a comparator for a record or sum is
   built where it is used, from the declaration, with runtime combinators
   (`__bynkCmpRecord`, `__bynkCmpSum`, `__bynkCmpOption`, `__bynkCmpList`, and
   `__bynkCmpFix` for a recursive type), rather than emitted once beside the type
   as `__bynkCmp_Priority`. That keeps witnesses working across units, including
   a type declared in a third unit and the context rebrand of a commons type,
   without exporting, importing and re-branding a comparator per type. The
   declaration is already in the checker's table for an imported type.
5. **A bounded generic used as a value (E).** `xs.map(top)`, or passing `top` as
   an argument, is instantiated at the reference: its type arguments come from
   unifying its parameter types with the expected function type. It emits a
   closure that captures the witnesses. If a type argument cannot be resolved the
   reference is `bynk.generics.ambiguous_instantiation`. Today every generic
   function used as a value is `bynk.generics.uninferable_type_arg`; this slice
   instantiates the `Ord`-bounded ones and leaves the rest as they were.
6. **Operators (F).** `< <= > >=` accept two operands of the same `Ord` type. A
   base type other than `Bytes` keeps the bare JS operator, byte-identical; every
   other `Ord` type, `T: Ord` and `Bytes` included, emits `cmp(a, b) < 0`.
   `==` keeps its structural meaning and emission.
7. **`Float` under the total order (G), an exception to track §4.** `sortBy`,
   `min`, `max` and `sort` on `Float`, or on a type containing a `Float`, use
   `__bynkCmpFloat`: `NaN` last, `NaN` equal to itself, `-0` equal to `0`. `max`
   returns `NaN` when one is present and `min` only when every element is `NaN`.
   Kernel `Float` keys therefore change emission, and change behaviour only when
   a `NaN` is present (the bare `<` comparator is inconsistent there). The
   operators keep IEEE (`NaN < x` is `false`). This breaks track §4's "code that
   does not use bounds must emit byte-identically" for `Float`-key kernels only,
   deliberately: one canonical order for a type outweighs a `NaN`-only golden
   change, and the alternative (G2) would leave `xs.sortBy((x) => x)` and
   `sort(xs)` disagreeing at `NaN`. No existing golden carried a `Float` key.
8. **Kernel keys widen; stdlib functions are Bynk (H).** The `List` kernel
   `sortBy`/`min`/`max` key rule is "`K` is `Ord`", through the structural check;
   `key_not_orderable` stays the kernel-position wording. Kernel lowering branches
   on the key type: `Int`, `String`, `Duration`, `Instant` and refined types over
   them keep the bare `<`/`>`; `Float` (G), `Bytes`, `T: Ord`, user types,
   `Option` and `List` use a comparator or the in-scope witness. `Query` is
   unchanged: its `sortBy`/`min`/`max` keep the five-base-type rule as its own
   predicate, because storage ordering has no SQL lowering for a user-type
   comparator. `bynk.list` gains `sort`, `min` and `max`, written in Bynk over the
   widened kernels (`xs.sortBy((x) => x)`): the first stdlib bounded generics and
   the end-to-end proof of witness passing. No other first-party commons exports
   a `sort`, `min` or `max`, so the free functions collide with nothing in the
   standard library; the kernel method `xs.min(key)` is unaffected.
9. **Fixes (I).** `bynk.generics.missing_ord_bound` is reported for `<`, an
   ordering kernel key, or a call into an `Ord`-bounded callee on a type
   parameter not declared `Ord`, once per parameter per function; its fix is
   machine-applicable (insert `: Ord`, ` + Ord`, the `where` entry, or replace an
   existing `Eq` with `Ord`) and `bynk check --fix` applies it. On a user type
   without `derives Ord` the error is `bynk.types.not_orderable`, whose "add
   `derives Ord`" suggestion is `MaybeIncorrect`: offered as a quick fix, never
   taken by fix-all or `--fix`, because opting in makes declaration order
   meaningful and that must be a person's decision. `Applicability` gains
   `MaybeIncorrect` for it.
10. **Reordering (J).** Reordering a `derives Ord` type's fields or variants
    changes its sort order. The wire contract hash ignores field and variant order
    and still does, since order matters only in memory; the reference says so. No
    contract-hash or diagnostic change. A lint is a later increment.

**Consequences.**
- A user type, a generic, and a `bynk.list` call can be ordered. `Bytes` is `Ord`.
- Existing code that sorts `Float` keys with a `NaN` now sorts them last instead of
  in an unspecified order; no other existing program changes behaviour.
- Emission changes only for `Float`-key kernels and the new `bynk.list` functions
  (every module that imports `bynk.list` also emits the three new functions);
  every other golden is byte-identical, and `compare.ts` is imported only by a
  module that uses a comparator.
- The constraint table has `Eq` and `Ord`; the satisfaction code
  (`has_eq`, the scope, `require_eq`) is now per constraint. `Key` and `Codable`
  follow in slices 2 and 3.
- The comparator for a user type is rebuilt at each use. A caller in a hot loop
  can bind it once; a later increment may hoist it.
- Editor surface (ADR 0156): hover on `derives` and `Ord`, completion of
  `derives` after a type body and `Ord` after `derives`, `T:` and `where A:`,
  `derives` as a keyword token, signature help showing `Ord` bounds and never the
  hidden comparator parameters, and quick fixes (`missing_ord_bound` is applied by
  fix-all; "add `derives Ord`" is offered and never applied by it).

Proved by:
- behavioural positives `2642_ord_surface_behaviour` (the surface and every
  orderable kind), `2643` (cross-unit), `2644` (recursion and mutual recursion
  through a bounded generic), `2645` (bounded methods), `2646` (value
  references), `2647` (a generic `derives Ord` type at two instantiations),
  `2648` (recursive types) and `2649` (`Float` `NaN` and `-0`, with `<` unchanged),
  each run with `expected_run.txt`;
- positives `2016_bytes_sortby_key` and `2016_bytes_min_key`, flipped from negative;
- negatives `2650`–`2661` (an undeclared record, an unbounded parameter at an
  operator and at a call, a non-orderable field, `derives Eq`, an unknown name,
  `Bool`, a record `sortBy` key, an unorderable argument, an ambiguous value
  reference, an opaque type outside its commons, an imported record);
- the tree-sitter grammar, corpus and cross-parser conformance cases, the
  formatter, and the editor-tooling tests.
