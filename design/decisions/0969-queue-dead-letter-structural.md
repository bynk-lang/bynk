# 0969 — The dead-letter consumer check uses the producer check's comparison

- **Status:** Accepted (v0.704.1)

**Context.** #3877 compared the two consumers' message types as display strings, so `Job` and a record with the same fields under another name were reported as different, while #3879's producer check, which uses ADR 0714's comparison, accepted the same pair. The two checks could disagree about whether two queue message types match.

**Decision.** `bynk.queue.dead_letter_type_mismatch` is decided by `queue_producers::message_compat`, with the source consumer's type as the sent type and the dead-letter consumer's as the taken type. A type with the same fields under another name, and a dead-letter consumer of a weaker refinement than the source's, are accepted; a different type is refused, with the comparison's findings as notes. The phase now runs with the other whole-program call checks (`phase_bundle_composition_unprovided`), which have the units' `uses` and `consumes` each consumer's type namespace is built from, so it runs once the per-unit checks are clean, as the producer check does.

**Consequences.** A project whose dead-letter consumer takes the same fields under another name no longer errors. The diagnostic's text is unchanged apart from the added notes. Hover, completion, semantic tokens and signature help are unchanged.
