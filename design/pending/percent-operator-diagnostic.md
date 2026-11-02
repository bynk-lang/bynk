---
level: minor
changelog: "`%` is a targeted error, `bynk.parse.no_remainder_operator`, that points at `Int.rem` and `Int.mod` and fixes simple operands to `.rem`; Bynk still has no `%` operator (#2804)"
probes: diagnostic_coverage
---

## ADR: no-remainder-operator
title: Bynk has no `%` operator; `%` is a diagnostic that points at `rem` and `mod`
summary: Diagnostic only, chosen by the maintainer 2026-11-02

**Context.** A program ported from a C-family language writes `a % b`. Until #2807 it lexed
as `bynk.lex.unexpected_character`, which says nothing about `Int.rem` and `Int.mod`
(ADR 0660, K6b). ADR 0660 left a `%` operator "possible later as grammar work".

**Decision.** Diagnostic only, chosen by the maintainer 2026-11-02. There is no `%` operator
and none is added. `%` lexes as a token (`TokenKind::Percent`) so recovery does not call it a
stray character. In binary-operator position at the multiplicative level the parser raises
`bynk.parse.no_remainder_operator` at the `%`, explaining that `a.rem(b)` takes the dividend's
sign (C-family) and `a.mod(b)` the divisor's. When the left operand is a name and the right a
name or integer literal, the error carries a quick fix (`MaybeIncorrect`: the parser cannot see types, and only `Int` has `rem`) rewriting the expression to
`a.rem(b)`; any other operand shape gets the explanation without a fix. `%` anywhere else is
an ordinary "expected an expression".

**Consequences.** The error aborts the enclosing declaration like any syntax error, and the
rest of the file is still checked (#1663, #1710). The fix assumes `Int` operands; on a `Float`
it leads to a missing-method error, which is the honest answer since only `Int` has `rem`.
A `%` operator can still be added later as sugar over `rem` or `mod`.
