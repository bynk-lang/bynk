# 0897 — A capability operation's type parameter is inferred from its arguments, `Events.emit` included

- **Status:** Accepted (v0.647.1)

**Context.** `Events.emit[E](event: E)` was refused as `bynk.generics.uninferable_type_arg` without a written
`[E]`, though its argument determines `E`. Capability-operation inference ran only for operations with
`Codable` parameters (`Idempotency.remember`, `Queue.sendValue`), and the docs disagreed with each other (#4113).

**Decision.** Inference runs for any capability operation with type parameters; an operation whose arguments
do not determine every parameter (`dedup[T](key)`) still needs it written. The owner-only check
(`bynk.event.emit_outside_owner`, `bynk.event.emit_not_an_event`) moves into one function and runs on the
type argument whether written or inferred, so inference cannot step around it. The checker records the emitted
event on the call's `Callee::Capability`, and the emitter and the architecture model both read it there, so a call
with no written `[E]` is emitted and listed like one with it. A type that is not named at all (`Events.emit(42)`,
a container, a type parameter) is `bynk.event.emit_not_an_event`. The "never inferred" passages are corrected, and the
duplicated `claim` subsection in the `bynk` capabilities page is merged into one.

**Consequences.** `Events.emit[E](e)` is unchanged and `Events.emit(e)` now compiles. A program that did not
compile now does, so nothing breaks.
