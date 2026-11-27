---
level: patch
changelog: The workerd system tests no longer collide on a port when two test modules ask for ports at once, and wrangler no longer fetches Request.cf from Cloudflare at start-up in tests
---

## ADR: workerd-test-port-claims-are-process-wide
title: The test port allocator's claim file is the process-wide record of a port
summary: Claims are created atomically and refuse a port this process already holds; wrangler is told not to fetch Request.cf

**Context.** #4578: after #4479 one `workers_runtime_smoke` run in twenty still failed with `Address already in use (localhost:29665)`, a port from the allocator's own band. The harness (`wrangler/mod.rs`, with `support/ports.rs`) is included by `#[path]` into about fifteen modules of the one `it` binary, so each module has its own copy of the allocator and its own `issued` set, and `claim` skipped any claim file named for its own pid. Two tests in different modules could therefore each be given the same free port. Separately, `cron_failed` once never saw wrangler ready: miniflare fetches `Request.cf` through the machine's proxy at start-up and the fetch was cancelled.

**Decision.** (A) `claim` creates its file with `create_new`, so the file is the process-wide record: a port any copy of the allocator in this process already issued is refused, as is one another process holds. (B) `npx_command` sets `CLOUDFLARE_CF_FETCH_ENABLED=false`, so miniflare uses its placeholder `Request.cf` and start-up no longer depends on a network fetch; no test reads `Request.cf`. Not done: retries, skips, weaker assertions.

**Consequences.** 20 of 20 runs of the 157 workerd and browser tests passed at 16 threads on four cores, against 19 of 20 on main (the failure was the port collision). The ai_stream, postgres/hyperdrive and `stream_request_body` failures from #4578 did not reproduce on this machine, before or after, so their causes are not established here; the Hyperdrive hang fix (postgres.js 4.1.1, #4575) is already in main.
