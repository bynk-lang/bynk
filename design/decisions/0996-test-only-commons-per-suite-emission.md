# 0996 — A suite's test-only names are emitted over that suite's own commons

- **Status:** Accepted (v0.730.1)

**Context.** Checking was scoped to a suite's own test-only commons (#4348), but the emitted test module is one module per target and was lowered over the union of every suite's. Two suites using different test-only commons that declare the same name (a function, a type, a constant) then crashed emission ("re-typed for emission reported errors the check pass did not"), and in a stub class two suites' same-named names were bound twice and failed `tsc` (#4373).

**Decision.** A case or property runner is lowered over the target's `uses` plus only the test-only commons its own suite `uses`; module-level imports keep the union, since the module imports every commons. A stub clause's value is lowered over its declaring suite's view, and its test-only commons' names are bound in a block of their own around that clause, not once for the method, so two suites stubbing the same capability each resolve `helper` to their own. The resolved stub records each clause's declaring file and that suite's `uses` for this. The same record fixes #4348's stub view, which took the first clause's file for every clause of the capability.

**Consequences.** The checker's and the emitter's views of a suite now agree, so the emit-time re-check cannot disagree with the check. A collision between a test-only commons' name and the target's own is unchanged: the target's wins, as for any commons.
