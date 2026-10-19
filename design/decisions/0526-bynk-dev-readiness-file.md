# 0526 — `bynk dev --ready-json` signals readiness with a file written once every context answers

- **Status:** Accepted (v0.366)

**Context.** The end-to-end testing track (§3.5, #2138 DECISION F) runs `system` suites against `bynk dev`. That needs to know when the Workers are up. `bynk dev` printed only a human table on stderr, and its `--format json` `serving` event (#2094) fires when the `wrangler dev`s are *started*, not when they answer. So `bynk-lang/shop`'s scenario script looped on a warm-up request.

**Decision.**

- **`--ready-json FILE`.** Once every served context answers a probe, `bynk dev` writes `{ "<worker>": "<url>" }` to `FILE`. The keys are the worker-directory names `--context` accepts, and the URLs are the ones the `serving` event reports.
- **The probe is a `HEAD /`, and any HTTP status counts.** Every emitted Worker has a `fetch`, so any status means it is up. `HEAD` is safe per RFC 9110, so a `GET /` route with side effects is never tripped. The probe runs from the existing 500 ms watch loop, with a 300 ms timeout.
- **A probe that can't succeed isn't silent.** Each context is probed only until it first answers. `bynk dev` warns at start when `--ready-json` meets `-- --local-protocol https` or a non-loopback `-- --ip`, which the plain-HTTP localhost probe can't reach. After 30 seconds without readiness, it notes once which contexts haven't answered.
- **The file's lifecycle is the session's.** The file is written once, atomically (to a temporary file, then renamed). It is removed at start, so a stale file is never read as ready, and again when the session stops. A failed write is reported once and not retried; the session keeps serving.
- **No new lifecycle event.** The `--format json` stream is unchanged. The file is the readiness surface, which a runner can wait on without parsing a stream.

**Consequences.** Readiness means each Worker answers a request. It doesn't promise that the dev-registry Service Bindings between contexts have connected, because they converge shortly after start. `bynk test --against dev` (a later slice) and `bynk-lang/bynk-dev` can wait on the file. A runner that starts `bynk dev` itself must never forward `-- --remote`.
