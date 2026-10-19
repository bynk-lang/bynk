# 0533 — Under `--against`, an `expect` on an absolute `Instant` is a warning, not a refusal

- **Status:** Accepted (v0.371)

**Context.** Track §3.4 and §3.7 (#2138). In process the harness clock starts at the epoch and every participant reads it (ADR for #2137). A real runtime's Workers read wall-clock time, which the runner can't reach, so `advance` is refused remotely. A case can still compare an instant against a constant (`Instant.fromEpochMillis(0)`), which holds in process and fails against a real runtime, and no `--settle` retry helps.

**Decision.** A remote build warns, once per case at the first such `expect`, when a comparison's `Instant` operand isn't run-derived. An instant is run-derived when it comes from a binding the case got from an address (or a field of one, or one shifted by a `Duration`); one built from a constant, or a `let` bound to one, isn't. It is a warning because whether an `expect` really depends on the clock can't be decided exactly, and a refusal would block cases that are correct.

**Consequences.** In process nothing changes. The documented remedy is to compare instants the run produced (`expires > created`) rather than absolute ones.
