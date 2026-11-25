# 0988 — An email handler states what mail it acts on

- **Status:** Accepted (v0.724)

**Context.** A `from email` handler inherited `Sender` silently, so whether it acts on mail that failed DMARC was never written down. Mail arrives from the internet, which is closer to HTTP, where `by` is required, than to a queue. This is the first part of #3888, which goes on to let a handler narrow it with a refinement of `Sender` (`Dmarc(…)`).

**Decision.** (A) `by` is required on every email `on message` handler: `bynk.actor.missing_by_on_email`, with a fix-it inserting `by Sender`, which means what the handler meant before (any mail). A service-header `by` satisfies it.

**Consequences.** This is security-relevant in intent but not in behaviour yet: an existing handler admits the same mail as before, now visibly, and `by Sender` is the spelling to search for when auditing which handlers act on unauthenticated mail. Every `from email` source in the repository, the documentation and the tests is migrated. Email is experimental (#3328), so the break is cheap. Decisions B to E (refinements, rejection before the handler, the verdict at every test tier) follow in later PRs.
