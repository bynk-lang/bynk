---
title: "Appendix A — Planned features"
---
> [!WARNING]
> This appendix is **non-normative**. The features named here are design
> directions, **not part of the normative Bynk language**. They are not
> implemented in the version this specification defines, are subject to
> change, and impose no requirement on a conforming implementation. They are
> recorded only to mark intended direction.

Two directions are designed but not yet shipped:

- **Sagas** — long-running, multi-step workflows with compensation, coordinating
  effects across contexts.
- **Event replay** — re-delivering past events to a subscriber that joins late or
  recovers, with ordering across concurrent publishes. Events themselves have
  shipped ([§5.7b](/book/spec/static-semantics/#events)), range patterns in a
  `via schema(...)` clause included; replay has not.

Two named follow-ons extend [agent invariants](/book/spec/static-semantics/#541-invariants-v080)
(shipped runtime-checked in v0.80):

- **Static provable-violation analysis** — a compile-time error for a handler all
  of whose paths provably commit a state that violates an invariant; a layer on
  top of runtime checking (static *satisfaction* proving remains further
  deferred).
- **A general typed-agent-fault channel** — making an `InvariantViolation` (and
  every other uncaught agent fault, such as a non-exhaustive match) a
  caller-distinguishable fault envelope, rather than the present bare 500. The
  surface stays a *fault*, never a `Result` variant.

These are sketches of intent, not specifications: this appendix deliberately
states no syntax or behaviour for them. For the design rationale and the current
thinking on what is deferred, see
[Versioning & roadmap](/book/about/versioning-and-roadmap/#what-is-deferred-to-v1).
