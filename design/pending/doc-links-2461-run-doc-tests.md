---
level: minor
changelog: "A fenced `bynk test` example in a doc comment runs as a test case, reported at its own line in the source file"
---

## ADR: doc-examples-run-as-cases
title: A doc-comment example marked `bynk test` runs as a case on its unit
summary: Marked doc examples are compiled into the run as synthetic cases, with line-preserving synthetic files

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

**Consequences.** A project with marked examples compiles three times (the
project, the check, the run); a project without them is unchanged. The synthetic
file name never shows. Grouping doc tests in the test explorer is a later slice.
