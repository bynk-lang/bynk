---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: "List patterns: `[]`, `[p, …]`, `[p, …, ..]` and `[p, …, ..rest]` match a `List` by its length and elements in a `match`, an or-pattern, after `is` and in `let … else`. Exhaustiveness is by length, so `[]` and `[x, ..rest]` need no `_`, and a missing length is reported as a pattern (`[_, _, ..]`). Binding `..rest` copies the remaining elements. A `..` followed by a literal is reserved for range patterns (patterns track slice 4, [#2680](https://github.com/bynk-lang/bynk/issues/2680)). New diagnostics: `bynk.parse.list_rest_not_last`, `bynk.parse.range_pattern_unsupported` and `bynk.types.list_pattern_on_non_list`"
---

## ADR: list-patterns
title: A list is matched by length and prefix, and `..` is a rest marker unless a literal follows it
summary: List patterns `[]`, `[p…]`, `[p…, ..]`, `[p…, ..rest]`; a rest marker that is last and takes a name or nothing; length constructors split by the column's rows in the usefulness engine; an if-chain lowering with `length` and index tests and a `slice` for `..rest`; and the rule that decides `..` between a rest marker and a range, shared with slice 5; realises patterns-track slice 4

**Context.** A list could be inspected only through `first()`, `get(i)` and
`length()`, so the checker could neither prove that `[]` and a non-empty case
cover every list nor bind the head and the tail. A `match` on a list was
`bynk.types.match_non_sum_discriminant`. Slice 3 (ADR 0680) made coverage one
usefulness engine with `Constructor` as the extension point, and named list
lengths as the first constructor to add. `..` is already the rest marker of a
record pattern (ADR 0669) and of an event pattern, and slice 5 plans to reuse it
for range patterns (`lo..hi`, `lo..`, `..hi`, jointly with events, #990).

**Decision.**

- **A. A `[` begins no other pattern.** Checked against both parsers before
  building: `[` could not begin a pattern, a `match` arm or a `let` pattern;
  `x is [ … ] { … }` closes the pattern at the `]` before the `if` block; and
  inside a list pattern only a name, `,` or `]` may follow a rest `..`. A list
  pattern therefore needs no parentheses, after `is` or anywhere.
- **B. The rest marker is last, once, and takes a name or nothing.**
  `[p₁, …, pₙ, ..]` matches *n* or more elements and binds nothing;
  `[p₁, …, pₙ, ..rest]` binds `rest: List[T]`; *n* may be 0. A `..` anywhere but
  last is `bynk.parse.list_rest_not_last` (suffix forms such as `[.., last]` are
  a later proposal); its quick fix moves a bare `..` to the end only when nothing
  but wildcards follows. `.._` is not a form.
- **C. `..` is a rest marker or a range bound by what follows.** After `..`, an
  lowercase identifier, `,` or `]` (or nothing, in a record pattern) makes a rest
  marker; a literal, including a negative one (`..9`, `..-1`), makes the upper
  bound of a range. A capitalised name after `..` is neither (`..MAX`, a named
  constant, is a plausible range bound), so it is rejected and reserved for range
  patterns, as is every binding name that must start lowercase. This slice reserves the second: `..` followed by a literal is
  `bynk.parse.range_pattern_unsupported`, with a note pointing at range patterns.
  Slice 5 and the events slice (#990) take the range production over the same
  token without changing what `..` means here. The two cannot overlap, because a
  rest marker is never followed by a literal and a range bound always is.
- **D. Typing.** The scrutinee MUST be a `List[T]`; anything else (a record, a
  sum, a base type, a `Set`, a `Map`) is `bynk.types.list_pattern_on_non_list`.
  Each element is checked at `T`, and `rest` has type `List[T]`. A `match` on a
  list whose arms include a list pattern is `MatchKind::List`; one with none
  keeps `match_non_sum_discriminant`. A list pattern is irrefutable exactly when
  it is `[..]` or `[..rest]`.
- **E. Coverage is by length, in the existing engine.** `Constructor::List(n)` is
  exactly *n* elements and `Constructor::ListAtLeast(n)` is *n* or more, with the
  elements as columns. `Space::List` splits a column along the lengths its rows
  use: with *L* the longest prefix any pattern in the column spells out, the
  constructors are the lengths `0..=L` and "*L* + 1 or more". The split is
  linear in *L* and needs no new budget. A query "*n* or more" splits along the
  same lengths. Two witnesses for the same gap, `[a]` and `[a, _, ..]`, are
  reported as one, `[a, ..]`. `unreachable_arm` and the fill-arms fix come from
  the same engine; `[x, ..]` after `[_, ..]` is `unreachable_arm`, not a
  duplicate.
- **F. Lowering stays the if-chain.** A list pattern tests
  `xs.length === n` (exact) or `xs.length >= n` (a rest marker; no test for
  `[..]`), then each refutable element against `xs[i]`. Bound elements are
  `const`; `..rest` is `xs.slice(n)`, which copies the remaining elements, so
  binding it costs O(length − *n*) and the specification states that cost.
  `..` with no name copies nothing. `[..rest]` binds the scrutinee itself, with
  no copy: no `List` value is ever mutated in place (every operation builds a new
  array, the language has no element assignment, and the emitted types are
  `readonly`), so the sharing is unobservable. Flat `switch` matches and every
  existing golden are byte-identical.
- **G. `let` takes a list pattern, refutably.** `[` joins the openers of a
  `let … else`. Every list pattern but `[..]` and `[..rest]` can fail, so it needs
  the `else`. The `else`-less form stays record-only (ADR 0675): `let [..rest] = xs`
  is `let rest = xs`, and an `else` on `[..rest]` is `unreachable_arm` with no fix.
- **H. `is`, or-patterns and guards add no rules.** Alternatives bind the same
  names at the same types, so `[x] | [x, ..]` is legal and `[x] | []` is not. A
  guard falls through as for any arm. A refined element (`_ where P`) stays
  `match`-top-level only, as ADR 0253 has it.

**Consequences.**

- A list is taken apart by shape with the compiler checking every length is
  handled, instead of `length()` and `get(i)` with no coverage.
- Recursive code over `[x, ..rest]` is quadratic because `rest` copies; the
  reference says so and recommends the kernel folds for iteration.
- Slice 5 gets its `..` rule decided here. It adds an interval `Constructor` and
  a range production without touching list patterns.
- On one line, an arm body that ends in a name followed by a `[`-led arm needs a
  separator, since `x [` there reads as a type application; arms on separate
  lines or ending in a comma parse as before. The tree-sitter grammar loses the
  postfix precedence of a call with type arguments to keep both readings alive.
