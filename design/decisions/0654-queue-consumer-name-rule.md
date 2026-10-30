# 0654 — A queue consumer's name follows Cloudflare's queue-name rule

- **Status:** Accepted (v0.439)

**Context.** `service … from queue("n")` only checked that `n` was non-empty, so a name Cloudflare refuses (uppercase, `_`, over 63 characters, a leading or trailing `-`) compiled and failed at `wrangler queues create`. The producer's `@name` (#2578) already applied the rule.

**Decision.** `bynk.queue.invalid_name` now also fires when the name fails `is_queue_name`, the predicate the producer uses. This is a language change (ADR 0023): a project with such a consumer name compiled before and is refused now.

**Consequences.** The two sides agree. `env_qualify`'s `-<env>` suffix counts toward the 63 characters but is not visible at compile time, so a name near the limit can overflow once qualified. No check catches that before `wrangler queues create`; that is left as a follow-up.
