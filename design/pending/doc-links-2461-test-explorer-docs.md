---
level: minor
changelog: "The VS Code Testing view tags a doc-comment example's case `doc-example`, so its tag filter groups the doc examples"
---

## ADR: doc-examples-test-explorer-tag
title: Doc-comment example cases carry a Testing view tag
summary: The extension recognises the doc example case name and tags the case, rather than adding a tree level

**Context.** Slice 2 of #2461 runs a doc-comment example marked `bynk test` as a
case named `doc example <file>:<line>`, so it already appears in the Testing
view under its unit's suite, linked to its fence. The proposal asks for doc tests
to appear as a group.

**Decision.** The extension tags such a case `doc-example` (recognised by the
name prefix the toolchain gives it), beside its tier tag, so the Testing view's
tag filter shows the doc examples together or leaves them out. It does not add a
tree level between a suite and its cases: every part of the extension that walks
a suite's children (the run plan, the file runs, the code lenses, the seeds)
assumes they are cases, and a case's id is the key the driver's `--suite`/`--case`
filters are planned from.

**Consequences.** No toolchain or run-document change: the case name is already
pinned. A tree node grouping the doc examples under a suite would be a larger
change to those walks and is left for later if the tag proves too little.
