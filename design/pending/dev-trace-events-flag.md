---
level: patch
changelog: `bynk dev --trace-events` writes one `request` event per request served, with the steps it took and their `.bynk` spans
---

`--trace-events` implies `--format json`. It builds the workers with per-request
trace recording (the emitter half is `dev-request-traces`), adds the
`nodejs_als` compatibility flag unless the project already lists `nodejs_compat`
or `nodejs_als`, and re-emits each `__bynk_trace:` line a worker prints as a
`request` event, with the fault's stack frames mapped to `.bynk` locations.
Without the flag, `bynk dev` output is unchanged.
