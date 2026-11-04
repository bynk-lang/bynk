# 0732 — A lambda parameter may be `_`

- **Status:** Accepted (v0.506)

**Context.** A callback often ignores an argument (`r.mapErr((e) => 0)`), and
patterns already have `_`. A lambda parameter had to be an identifier, so the
author invented a throwaway name that reads as if it were used. `_reason` cannot
stand in: identifiers do not begin with `_`, so it lexes as `_` followed by
`reason` — by design, and unchanged here.

**Decision.** `_` is accepted as a lambda parameter name, with or without a type
annotation (`(_: Int) => 1`), in the hand-written parser and the tree-sitter
grammar. The parser gives it the hidden name `__wild<offset>` with an empty span
at the `_`, the same device as the `__lambdaArg<offset>` binder of pattern
parameters (ADR 0484): the checker types it from the expected function type
like a named parameter, the emitter gets a unique valid TypeScript name for each
`_` (so `(_, _) => …` has no duplicate identifiers), and the empty span keeps
hover, rename and unused-binding lints off it. No user identifier can spell the
hidden name, so the body cannot reach it; `_` in expression position stays
`bynk.parse.expected_expression`. `_` may repeat in one list. The formatter
prints `_` back.

**Consequences.** Additive: no existing program changes meaning. `_reason` is
still a parse error; its message now points at `reason`, the token the parser
stops at, rather than at `_`.
