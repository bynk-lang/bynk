---
level: minor
changelog: A `from queue("…")` consumer's name is checked against Cloudflare's queue-name rule (1–63 lowercase letters, digits or `-`, starting and ending alphanumeric); names Cloudflare would refuse at `wrangler queues create` are now a compile error
---

## ADR: queue-consumer-name-rule
title: A queue consumer's name follows Cloudflare's queue-name rule
summary: Consumers and producers share one queue-name predicate

**Context.** `service … from queue("n")` only checked that `n` was non-empty, so a name Cloudflare refuses (uppercase, `_`, over 63 characters, a leading or trailing `-`) compiled and failed at `wrangler queues create`. The producer's `@name` (#2578) already applied the rule.

**Decision.** `bynk.queue.invalid_name` now also fires when the name fails `is_queue_name`, the predicate the producer uses. This is a language change (ADR 0023): a project with such a consumer name compiled before and is refused now.

**Consequences.** The two sides agree. `env_qualify`'s `-<env>` suffix counts toward the 63 characters but is not visible at compile time, so `bynk deploy` still refuses a name that overflows once qualified.
