# 0522 — A `system` case drives cron and queue handlers through the Worker's `scheduled` and `queue` entry points

- **Status:** Accepted (v0.362)

**Context.** ADR 0207 D1 admitted only `http` to `system`, because only `worker.fetch` was wired, and named queue-at-system a follow-on. A `system` case could not run a cron or queue handler through the deployable Worker, and #2131 refused the addresses until they could be driven. The end-to-end testing track (§3.3, #2136) settles how.

**Decision.**

- **A — `queue` is a serialisation edge; `cron` is not.** `bynk.tier.system_needs_wire` admits a target with an `http` or a `queue` service. A queue message crosses the platform's serialisation, so a queue-only, single-context target qualifies. A scheduled event carries only a cron string and a time, so a cron-only target still doesn't, but a cron address is legal in a suite that already qualifies.
- **B — the address yields the handler's own result**, so the body promotes unchanged. `q.message(m)` calls `queue(batch, env)` with a one-message batch whose `ack()` / `retry()` the harness's double records, and yields `Ack` or `Retry(reason)`, with the reason the entry logs. `svc.schedule("<expr>", at)` calls `scheduled({ cron, scheduledTime: at }, env, ctx)`, and the test build's entry for a context a `system` suite targets stores the run's `Result` on the execution-context double, which the driver reads back. A deployable build carries no suites, so its entry is byte-identical to before.
- **C — the queue body takes a JSON round trip.** The driver serialises the message as the producer does, then `JSON.parse(JSON.stringify(…))`, so a type that doesn't survive the round trip fails as it would on the platform.
- **D — the scheduled time stays required** when the handler declares it, as at `unit`. A handler without it is fired at `0`.
- **Faults map as the platform would.** A consumer that throws is retried, so the address yields `Retry`. A cron handler that throws has no `Result`, so it fails the case, as a throw does at `unit`. Neither crashes the runner. Both drivers settle the execution context (ADR 0478) before they return.
- **One harness per case.** Every driver in a `system` case reaches the harness the case built, so state an earlier address leaves behind is what a later one reads. Each case still starts fresh (ADR 0153 D7). Before this, each http driver built its own harness, so a later call in the same case saw none of an earlier call's agent state.

**Consequences.** `bynk.test.schedule_needs_in_process` and `bynk.test.message_needs_in_process` are retired. ADR 0207's queue-at-system follow-on is closed. A shop-style scenario (a sweeper run, then a read over the wire; a delivery counted once however often it is delivered) is a `system` case.
