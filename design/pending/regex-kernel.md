---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: "`String` gains `matchesRegex`, `containsRegex`, `findRegex`, `findAllRegex`, `replaceRegex` and `splitRegex`, each over a string-literal pattern checked at compile time exactly as a `Matches` pattern is (syntax and ReDoS), with leading inline `(?i)`/`(?m)`/`(?s)` flags; a non-literal pattern is `bynk.types.regex_not_literal` (standard-library K6c, [#2600](https://github.com/bynk-lang/bynk/issues/2600))"
---

## ADR: regex-kernel
title: Regex methods on String take literal patterns, checked as Matches patterns are
summary: six `String` methods match, capture, replace and split over JS `RegExp`, but only with a string-literal pattern, so the compile-time ECMAScript validation and ReDoS analysis that `Matches` patterns get (#724, #1651) cover every pattern; captures come back as `List[Option[String]]`, flags as a leading `(?ims)` group

**Context.** A service could only check a string against a pattern, through a
`Matches` refinement; it could not extract, rewrite or split. For `Matches`
the checker already validates the pattern with `regress`, an ECMAScript engine
in Rust, under the emitted `RegExp`'s semantics, refuses nested unbounded
quantifiers (#724) and runs an automaton ambiguity analysis (#1651), because a
backtracking `RegExp` on request input lets a crafted string stall a Worker.
JS `RegExp` has no timeout, and Workers bill and stop on CPU. A kernel cannot
return a library record, and a new built-in type is reserved-name work.

**Decision.**

- **A. Literal patterns only.** Each regex method's pattern must be a string
  literal; anything else is `bynk.types.regex_not_literal`. The literal then
  passes the `Matches` checks: `invalid_regex`, `catastrophic_regex`, and
  `polynomial_regex`, with no length bound to excuse a polynomial pattern
  (a method runs on any string). **Rejected:** run-time patterns with a
  run-time guard, which JS cannot enforce.
- **B. Six methods.** `matchesRegex` (whole string, anchored `^(?:…)$` as
  `Matches` is) and `containsRegex` (anywhere) `-> Bool`; `findRegex ->
  Option[List[Option[String]]]` and `findAllRegex -> List[List[Option[String]]]`
  (element 0 the whole match, then each group, `None` for one that did not
  take part; `findAllRegex` is `matchAll`'s non-overlapping, left-to-right
  scan); `replaceRegex(p, with)` with JS replacement syntax (`$1`, `$<name>`,
  `$&`, `$$`), deliberately unlike `replace`'s literal insertion (#1650);
  `splitRegex`, JS `split`'s algorithm without the captured groups it would
  splice in.
- **C. JS `RegExp`, `Matches`' flag set, inline flags.** No `u` flag, as
  `Matches` uses, so `.` and classes work on UTF-16 code units, the string
  kernel's unit. A pattern may open with one `(?flags)` group of `i`, `m` and
  `s`, each at most once; the checker validates the pattern with those flags
  and the emitter passes them to `RegExp`. JS lacks inline flags, so this is
  a Bynk convention (as in Python, Rust and PCRE). **Rejected:** a flags
  argument on every method, and `u`/`v`, which change class and escape
  syntax.
- **D. Lowering.** `new RegExp(source, flags)` built from the checked literal,
  with `g` added for `findAllRegex`/`replaceRegex` and `y` for the sticky
  `splitRegex` scan. V8 caches compilation by source and flags, so each
  pattern compiles once; a fresh object per call keeps no `lastIndex` state.
  (The proposal's module-level constant would have needed new hoisting
  machinery for no compilation saving.)
- **E. Stable.**

**Consequences.** Every regex in a Bynk program, refinement or method, has had
its syntax and backtracking cost checked before it runs. Ambiguity that only
case-folding creates under `(?i)` is not detected, since the analysis reads the
pattern as written. Named-group maps (`findNamedRegex`) can follow.
