# 0853 — Each sequence-model branch carries its arm's source span

- **Status:** Accepted (v0.625.2)

**Context.** An `AltBlock` had a span but its `Branch`es did not (#3584), so a client
could not link a branch to its source or mark it covered, and an empty branch had no
other span to borrow.

**Decision.** `Branch.span` is the arm's extent: for an `if`, the `then` branch runs
from the `if` keyword to the end of its body and the `else` branch is its block (the AST
records no `else` keyword span); for a `match`, the arm from its pattern to its end. On
the wire it is `WireBranch.range`, as for `WireMessage` and `WireAltBlock`. An else-less
`if` still emits a single branch (an `opt`) and no synthesised else, so no `synthesised`
flag is added (amended with the maintainer's coordinator).

**Consequences.** Additive: clients ignore unknown fields. No language surface changes.
