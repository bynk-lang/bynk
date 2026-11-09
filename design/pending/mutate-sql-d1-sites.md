---
level: patch
changelog: "`bynk test --mutate` makes SQL mutants only from the SQL the checker ties to a D1 call (including a `const` bound to one, in any case), and now reads `\\n` escapes and `CASE … END` correctly"
---

## ADR: mutate-sql-from-d1-sites
title: `--mutate` takes SQL mutants from D1 call sites, not a keyword guess
summary: The checker's D1 sites pick the SQL literals; the scan runs on decoded text

**Context.** ADR 0768 chose SQL literals by a keyword heuristic because linking
to D1 calls looked costly. It mutated prose that matched, missed lower-case SQL,
and a `\n` escape lexed as part of the next word. The checker now records each D1
call's SQL argument, and a `String` const bound to one (#3070, #3130).

**Decision.** `check_project` returns those sites (`ProjectCheck::d1_sql_sites`,
read from the checker's callee map; the checker is unchanged). A literal gets SQL
mutants only if it is such an argument, or the initialiser of a const a site
names (following const-to-const aliases, in the calling unit or a unit it
`uses`). The keyword heuristic is removed. The scan runs on the literal's decoded
value, mapped back to source offsets, so `\n` is whitespace and ends a `--`
comment; a `CASE … END` in a `WHERE` is one unit when splitting conjuncts.

**Consequences.** Prose never gets SQL mutants; lower-case and `const` SQL does.
`bynk test --mutate` now runs a project check first. A const reference reaches
its declaration by name within the calling unit and its `uses`, which can
over-include a same-named const of another unit; that yields extra mutants, not
wrong ones.
