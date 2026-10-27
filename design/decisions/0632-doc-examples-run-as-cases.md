# 0632 — A doc-comment example marked `bynk test` runs as a case on its unit

- **Status:** Accepted (v0.421)

**Context.** Slice 1 of #2461 type-checks every fenced `bynk` example in a doc
comment, but never runs one, so a wrong `expect` in an example still passes.

**Decision.** A fence whose info string includes `test` (`bynk test`, or
`bynk,test`) and whose body is statements is also run. After the check passes,
the project is compiled again with those cases added as a synthetic `suite` on
the declaring unit, so they are emitted and run like any other case, named
`doc example <file>:<line>`. A synthetic file has the same lines as the real
file, with each body line on its real line and column, so a failing `expect`
and a case's discovered location carry the real line and column; the run's
output (human, `json` and `ndjson`) shows the real file name in place of the
synthetic one. Unmarked examples stay check-only, and a marked fence of
declarations is checked but has no case.

A doc example's case is marked in the json and ndjson case records with an
optional `origin` field, `"doc"`, appended after the case's other optional
fields and absent for every other case (not `kind`, which a failed generative
case already uses for `"property"`/`"history"`; and not always emitted, which
would change every document, including a project's with no doc examples). A
reader treats a missing `origin` as an ordinary case. Consumers group on it, not
on the case name, which is display text.

**Consequences.** A project with marked examples compiles three times (the
project, the check, the run); a project without them is unchanged. The synthetic
file name never shows. Grouping doc tests in the test explorer is a later slice.
