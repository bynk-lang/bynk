# 0986 — A test-only commons is in scope only where it is used

- **Status:** Accepted (v0.722.1)

**Context.** #3984 added a test-only commons to the target's view for every suite of that target, so a suite saw its names although it had no `uses` of it, and compiled only while a sibling suite imported it (#4348).

**Decision.** This amends ADR 0974 (test-only commons): its second limit, that a test-only commons was added to the target's view for every suite of that target, is lifted. The checks build one view per suite: the target's `uses` plus the test-only commons that suite `uses`, so a name is unknown in a suite that does not import it (`bynk.resolve.unknown_name`). The emitted test module is one module per target and still unions every suite's test-only `uses`; it only has to reach every name a checked suite may spell, so the union adds nothing to what a suite can write.

**Consequences.** A `stub … returns` value is checked in its declaring suite's view too, so a stub value may name what that suite's test-only commons declares and not what another suite's does; the stub class method binds a used test-only commons' names so the emitted value resolves them. `UnitTable` records whether a unit is test-only (`test_only`) for that.
