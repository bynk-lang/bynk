---
level: minor
probes: emit_diagnostics, diagnostic_coverage
changelog: A recursive generic type declares and works inside a context; it is refused only where a codec is required (`bynk.generics.recursive_generic_at_boundary`, now at the use with the field path), and polymorphic recursion is refused at the declaration (`bynk.generics.polymorphic_recursion`)
---

## ADR: recursive-generic-refused-at-use
title: A recursive generic type is refused where a codec is required, not at its declaration
summary: Uniform recursion declares and works in memory; polymorphic recursion is a declaration error; codecs are a later step

**Context.** ADR 0197 C refused a recursive generic type because it has no finite
set of monomorphised codecs, and ADR 0237 and ADR 0705 C carried the refusal to
sums and codec factories. The refusal was made at the declaration, because a
record field and a sum payload were treated as boundary positions, so
`type Tree[A] = | Leaf | Node(left: Tree[A], value: A, right: Tree[A])` was
unusable even in memory (and was reported once per self-reference, calling a sum
a "record"). The spec already said such a type "remains usable inside a context".
ADR 0197's own text separates uniform recursion (`Node[T]` inside `Node[T]`)
from polymorphic recursion (`Weird[List[T]]` inside `Weird[T]`), which the
implementation lumped together. Proposed in #3192, from #3175.

**Decision.** (A) Follow the K-streams model (#2623): a record field, a sum
payload and a capability operation signature no longer refuse a recursive
generic; the refusal stays at every position that serialises (service and agent
handler signatures, an agent `store` field and key, an event field, an actor
identity type, `Json.encode`/`Json.decode` targets and `Codable` witnesses).
(B) A transitive use-site walk (`recursive_generic_path`, beside `stream_path`)
refuses a type that merely contains a recursive generic where that type is used,
and names the field path. (C) The diagnostic keeps its code
`bynk.generics.recursive_generic_at_boundary`, fires once per use, and takes the
noun (`record` or `sum`) from the declaration. (D) Recursion is admitted when it
is uniform: each recursive edge, including through a mutually recursive partner,
passes the declaration's own parameters unchanged and in order. Any other
recursive edge is refused at the declaration with the new
`bynk.generics.polymorphic_recursion`; this is conservative (it also refuses
`Pair[B, A]` inside `Pair[A, B]`) and may be relaxed later. (E) A codec for a
recursive generic is a separate, later increment, and codec factories are out of
scope here. This partly supersedes ADR 0197 C, and the premise of ADR 0756 G
("a recursive generic record is already refused in a record field") no longer
holds; a self-referential default is bounded by that ADR's depth limit instead.

**Consequences.** Trees, non-empty lists and expression types over a type
parameter work in a `context` and a `commons` with no codec. The emitter is
unchanged: a generic sum already emits as an erased TypeScript union, proved by
a behavioural fixture whose output builds, folds and runs. The emit-side
`recursive_generic_names` short-circuit still gates codec generation, which the
checker still refuses at every position that needs one. The editor surface is
unchanged (hover, completion, semantic tokens and signature help), apart from
the new diagnostic's message carrying the field path. Existing fixtures that
expected the declaration-level error moved to use-site expectations.
