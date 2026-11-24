# 0892 — Every bynk.list function has a List method

- **Status:** Accepted (v0.643)

**Context.** ADR 0116 moved `List`'s vocabulary into methods and deprecated the
free functions, but left `reverse`, `find`, keyless `sort`/`min`/`max` and
`traverse` with no method form, so `bynk.list` was still the only home of
them. `contains`, `last`/`lastOrElse` and `isEmpty` existed nowhere (#3828).

**Decision.** `ListOp` gains `reverse`, `find(p)`, `contains(x)`, `last`,
`lastOrElse(d)`, `isEmpty`, `sort()`, `traverse(f)`, and a zero-argument form of
`min`/`max`, dispatched on argument count beside the keyed forms. The keyless
ordering forms need the element's `Ord` witness (`bynk.types.not_orderable` for
a type that is not orderable, `bynk.generics.missing_ord_bound` on an unbounded
type parameter); `contains` needs `Eq` (`bynk.generics.missing_eq_bound`).
`traverse` is sequential: `forEach` that keeps the results. A concurrent form
that returns values is #3967's. Every `bynk.list` function now warns with
`bynk.list.deprecated_function` and a machine-applicable fix to its method;
`find` rewrites to `xs.find(p)` rather than `filter(p).first()`. The module's own
bodies delegate to the methods and no repository source uses it. Removal is a
later step.

**Consequences.** Existing `bynk.list` users get new warnings, never errors: the deprecations are warnings-only (no `bynk check` or `bynk build` mode fails on warnings), so there is no upgrade step and no `breaking:` entry. The
checker keeps the existing `bynk.list.deprecated_function` code rather than a
new `bynk.deprecated` one.
