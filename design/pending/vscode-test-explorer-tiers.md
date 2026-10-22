---
level: patch
changelog: The VS Code Test Explorer tags each test with its tier (`unit`, `integration`, `system`) and adds **Run against…**, which runs the `system` tests against `dev` or a `[test.targets]` entry; `bynk test --format json` gains an optional per-case `tier`
---

## ADR: test-json-optional-trailing-case-fields
title: The test JSON appends optional case fields, and the driver supplies the tier from the compile
summary: A case's effective tier rides the `--format json` document as an optional trailing field

**Context.** A case's tier is per case (`as unit | integration | system` on the
`case` or `suite` header, ADR 0153). The `--format json` document (ADR 0098)
carried only the suite `kind`, which the emitted runner derives from the module
name (a file-naming convention), so a `system` case in an ordinary file was
indistinguishable from a unit case, and the Testing view could not filter by
tier or run only the `system` suites. Tooling that guessed the tier from the
source would drift from the checker, which already holds each case's effective
tier (`case_effective_tier`).

**Decision.** `Case` gains an optional `tier` (`"unit"`, `"integration"` or
`"system"`), appended after the existing fields and omitted when absent, so a
document that does not carry it keeps its byte layout (ADR 0098's field-order
rule). Further optional trailing fields follow the same rule. The driver attaches
the tier from the compile's case table, keyed by suite and case name, in
discovery, a run, a coverage run and a merged `--against dev` run; the emitted
runner and its golden fixtures are untouched. A case with no tier (a property or
a contract attack) omits the field. A remote run's skipped cases are `system` by
construction and say so.

**Consequences.** The Test Explorer tags each case from the document and falls
back to the suite `kind` for a driver that predates the field. Consumers must
ignore fields they do not know. A later case field (a Slice C concern) is
appended the same way.
