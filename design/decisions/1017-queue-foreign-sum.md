# 1017 — A queue consumer's sum of Foreign senders resolves by the message's signer, through the call-sum seam

- **Status:** Accepted (v0.751)

**Context.** Part of #3899. A consumer that admits several foreign producers needs to know which one sent a message, as an `on call` sum does for its callers. Without a seam the handler read a `who` nothing bound, so the checker refused any sum or binder on a queue consumer.

**Decision.** The seam is the `call` sum's, not a new one: `queue_admission_of` yields the members, `lower_actor_seam_ir` lowers them to `ActorSeamIr::CallSum`, the compose wrapper takes `__who` first and threads it into `deps` as `who`, and the entry builds it as `{ tag: __a.from }`, which the admission has already verified is one of the members (only admitted senders have keys). A sum that also names an unsigned actor stays refused (its messages carry no signature to tell them from the signed ones), and so does a lone `Foreign` actor with a binder, which identifies no one beyond itself. A case acts as a member with `by <Actor>` at the unit and `system` tiers, as for a `call`.

**Consequences.** `by Producer` consumers and a lone `Foreign` consumer are unchanged. The `Producer` refinement, whose identity is the producing context's name, reuses this seam in the next slice.
