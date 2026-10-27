# 0643 — Doc-comment example cases carry a Testing view tag

- **Status:** Accepted (v0.433)

**Context.** Slice 2 of #2461 runs a doc-comment example marked `bynk test` as a
case named `doc example <file>:<line>`, so it already appears in the Testing
view under its unit's suite, linked to its fence. The proposal asks for doc tests
to appear as a group.

**Decision.** The extension tags such a case `doc-example` (read from the `"origin": "doc"` the driver
puts on the case's record, never from its name, which is display text), beside its tier tag, so the Testing view's
tag filter shows the doc examples together or leaves them out. It does not add a
tree level between a suite and its cases: every part of the extension that walks
a suite's children (the run plan, the file runs, the code lenses, the seeds)
assumes they are cases, and a case's id is the key the driver's `--suite`/`--case`
filters are planned from.

**Consequences.** No toolchain change in this increment: the `origin` field is the
one the run-doc-tests increment added. A tree node grouping the doc examples under a suite would be a larger
change to those walks and is left for later if the tag proves too little.
