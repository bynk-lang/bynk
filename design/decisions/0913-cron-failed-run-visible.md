# 0913 — A failed cron run is visible to the platform

- **Status:** Accepted (v0.658)

**Context.** The generated `scheduled` entry logged an `Err` with its whole payload and returned, so Cloudflare's Cron Past Events recorded a success. A throw escaped unlabelled, and an unmatched `event.cron` was dropped silently.

**Decision.** (A) Failure is marked by passing a rejected promise to `ctx.waitUntil` (`__bynkReportFault`), the documented route to the Past Events status; throwing from `scheduled` was rejected because it relies on undocumented behaviour, including retries. (B) An `Err` logs one structured record `{message, level, fields: {site, type, variant}}` and never the payload, matching ADR 0825. (C) A throw is caught, logged with a labelled stack, reported and rethrown, keeping the uncaught-exception outcome and ADR 0522's rule that a throw fails a case. (D) `ctx` is always in the `scheduled` signature. (E) The `default:` arm warns once, naming the unmatched expression. (F) The test double records a cron `Err` report without failing the case or counting a detached failure; a throwing run fails its case once.

**Consequences.** In a Worker group a member's own entry applies the same wrapper, so no group-specific code was needed; Past Events records only the first failed `waitUntil`, while the logs name every failing member. Verified locally on workerd: the scheduled trigger answers `500 exception` for an `Err` run and a throwing run and `200 ok` for an `Ok` run.
