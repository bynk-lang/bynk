# 0768 — `--mutate` SQL literal mutants

- **Status:** Accepted (v0.525.7)

**Context.** Proposal decision C2 deferred SQL mutants until the SQL literal's
structure was exposed by the front end, and noted the checker no longer guards
text inside a string. The front end still exposes no SQL structure.

**Decision.** Sites are found lexically, inside the source text of a string
literal whose first word is a statement or clause keyword (`SELECT`, `INSERT`,
`UPDATE`, `DELETE`, `REPLACE`, `WITH`, `FROM`, `WHERE`), skipping quoted SQL
strings and `--` comments. The mutations are `<`↔`<=`, `>`↔`>=`, `max(`↔`min(`,
and dropping one top-level `AND` conjunct of a `WHERE` with two or more (a
`WHERE` containing `OR` is skipped, `BETWEEN … AND …` is one conjunct). Validity
needs no new check: D1 SQL is already prepared against the migrations' schema at
compile time (#3070), so a mutant that no longer prepares is `invalid` and
unscored. Where a database has no migrations nothing is checked and such a
mutant simply runs. Consts bound to SQL literals are mutated where they are
declared. Not mutated: `=`↔`<>`, `AND`↔`OR`, literals and `LIMIT`.

**Consequences.** More mutants per project; `--only`/`--max-mutants` bound
them. No grammar, checker, emitter or runtime change.
