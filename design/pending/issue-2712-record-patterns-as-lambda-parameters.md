---
level: minor
probes: ts_writes, diagnostic_coverage
changelog: "Record patterns as lambda parameters: `items.fold(0, (total, Item { qty, price, .. }) => total + qty * price)` takes an irrefutable record pattern in place of a lambda parameter's name, mixed freely with name parameters, with the parameter's type from the expected function type or the record's own name; a refutable or non-record pattern, or an annotation, is a parse error (patterns track slice 6, [#2712](https://github.com/bynk-lang/bynk/issues/2712))"
---

## ADR: record-patterns-as-lambda-parameters
title: A lambda parameter may be an irrefutable record pattern, desugared to a hidden parameter and an `else`-less `let`
summary: `(Item { qty, .. }) => …` takes the place of a lambda parameter's name; the parser rewrites it to a hidden `__lambdaArg<offset>` parameter plus slice 2's plain `let`, so the checker, emitter and every slice 1–2 diagnostic are reused; function signatures stay plain names; realises patterns-track slice 6

**Context.** A lambda parameter was a name with an optional annotation
(`LambdaParam { name, type_ref, span }`), so a fold or `map` over records read
fields one `.field` at a time, or opened a block with `let Item { … } = it`
(slice 2, #2654). The kernel folds are the recommended way to iterate, and their
lambdas are where record fields are read most. The patterns track
(`design/tracks/patterns.md` §5 Q5) left lambda parameters as an optional later
slice; the maintainer asked for it (#2712). Function parameters stay a non-goal.

**Decision.**

- **A. The parse is unambiguous (spike, recorded in the PR).** `lambda_ahead`
  only counts parentheses, so `(T { …, .. }) =>` and `(a, T { …, .. }) =>` are
  already lambdas. A record pattern's `..` is not an expression token (spread is
  `...`), so no parenthesised expression can contain one. In tree-sitter,
  `lambda_param` gains a record-pattern branch restricted to binding fields, at
  the cost of two declared GLR conflicts (`field_init`/lambda record field, and
  `_primary`/lambda field binding) that the next token settles. Tree-sitter
  cannot see capitalisation, so it accepts `status: Paid` there; the compiler
  refuses it, and the conformance test excludes that one negative fixture by name.
- **B. Which patterns.** Only an irrefutable record pattern, judged by
  `Pattern::is_irrefutable`: every field a shorthand binding, `f: name`, `f: _`
  or a nested irrefutable record pattern. A refutable one is
  `bynk.parse.lambda_pattern_refutable`, naming the first field that can fail
  and suggesting a `match` in the body; any other pattern is
  `bynk.parse.lambda_pattern_not_record`. A lambda has nowhere to put an `else`.
- **C. No annotation.** `(Item { sku, .. }: Item) => …` is
  `bynk.parse.lambda_pattern_annotation`, with a machine-applicable fix that
  removes `: T`; the type name already states the type.
- **D. Typing.** A pattern counts as one parameter, so `lambda_mismatch` is
  unchanged. Against an expected function type the parameter takes the expected
  type and the desugared `let` checks the pattern against it, so slice 1's checks
  (field existence, duplicate fields, `bynk.context.opaque_inspection`) and
  `pattern_type_mismatch` apply unchanged and a generic record's type arguments
  come from the expectation. With no expected type a non-generic record is typed
  from its name, as if annotated, and counts as annotated for the generic-call
  inference rule; a generic one is `bynk.lambda.unannotated_param`, since nothing
  can supply its type arguments.
- **E. Lowering.** The parser rewrites `(P) => body` to
  `(__lambdaArg<offset>) => { let P = __lambdaArg<offset>; body }` (one rewrite
  per pattern parameter, the first outermost), the `let` being slice 2's plain
  destructuring. The binder's span is empty at the pattern. `LambdaParam.pattern`
  keeps the source pattern, and `LambdaExpr.block_body` records whether the body
  was a block, so the formatter peels the wrapper and prints the source form. No
  JS parameter destructuring is emitted (one lowering path); a pattern parameter
  is emitted with its checked type so a lambda held in a `let` compiles under
  `tsc --strict`. Lambdas without a pattern parameter emit byte-identically.
- **F. Hidden binders stay hidden.** No diagnostic, inlay hint, local, hover,
  semantic token or signature label names `__lambdaArg…` or `__letElse…`; a test
  pins it. An opaque-type violation in a `let` or lambda-parameter record pattern
  is now reported at the pattern ("cannot match fields of …") rather than as a
  `match` on the hidden binder.

**Consequences.**

- `items.map((Item { sku, .. }) => sku)` and the fold form read without a
  throwaway name or repeated `it.`, and adding a field to the record breaks none
  of them (every record pattern is open).
- A function, method or handler parameter is still a name; to destructure one,
  start the body with a `let` pattern.
- Both parsers and the formatter, completion (fields then `..` inside a pattern
  parameter), hover, semantic tokens and slice 1's missing-`..` fix cover the new
  position. There is no quick fix for a refutable parameter pattern: rewriting it
  to a name plus a `match` cannot be done reliably from the parser's view.
- A parenthesised match-arm guard followed by `=>` is already taken for a lambda
  by `lambda_ahead` (`n if (flag) => …`); this slice does not change that.
- This is the last implementation slice of the patterns track; its retirement
  review is next.
