# 0982 — A queue consumer cannot name a Producer refinement or a Foreign actor until delivery verifies it

- **Status:** Accepted (v0.719)

**Context.** Part of #3899. A consumer should be able to narrow who may send to it: a refinement of `Producer` admitting only named contexts, and a `Foreign` actor for a producer outside the project. Both need the signed message format and its verification, which is a later part of #3899.

**Decision.** (B, C) The declarations are accepted: `actor Shop = Producer where IsContext(…)` has the vocabulary of a `Caller` refinement, and `Foreign` is admissible on a queue consumer. Until delivery verifies them, naming one on an `on message` handler is `bynk.actor.queue_actor_not_enforced`, with a note to write `by Producer`. A consumer is never accepted with a restriction that nothing enforces. A `Producer` refinement is refused on every protocol but `queue` (`bynk.actor.scheme_not_admissible`), because nothing elsewhere would enforce its `IsContext(…)`.

**Consequences.** No existing program changes meaning. The error is lifted, and the negative fixtures `3899_producer_refinement_on_queue` and `3899_foreign_on_queue` become positive, in the part of #3899 that enforces delivery.
