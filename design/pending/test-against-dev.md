---
level: minor
changelog: `bynk test --against dev` runs the `system` suites, unchanged, against a local `bynk dev` it starts itself, one per distinct `env`. It applies each `env` and the declared secrets as `-- --var`, waits for the `--ready-json` file, accepts only loopback URLs, and stops each session by its own pid. `bynkc test --against dev` is refused, because only `bynk` can start `bynk dev` (#2138)
---

## ADR: test-against-a-local-bynk-dev
title: `bynk test --against dev` starts, waits for and stops a local `bynk dev` per distinct `env`
summary: The `bynk` driver hands `bynkc` the binary that starts `bynk dev`; each env group gets its own session; URLs loopback-only; stopped by pid, then process group

**Context.** End-to-end testing track §3.5 (#2138): "`bynk test --against dev` starts `bynk dev` with the suites' configuration, waits for readiness, runs the `system` suites unchanged against it, and stops it. Suites with different `env` get one `bynk dev` per distinct configuration, run in turn." `bynk test` delegates to `bynkc`, which owns the build and the runner but can't start `bynk dev`, which is the `bynk` driver's.

**Decision.**

- **The driver hands over the binary, not the session.** `bynk test --against dev` sets an internal `BYNK_DEV_COMMAND` (its own path) for the `bynkc` it delegates to, which orchestrates. `bynkc test --against dev` without it is refused, naming `bynk test`.
- **One session per distinct `env`.** A `--against dev` build reports the distinct `env` maps of its runnable `system` cases. For each map, the runner starts `bynk dev --ready-json` on free ports, passing the map's entries and a fixed test value for every other declared secret (the Workers' `bynk-secrets.json`) as `-- --var NAME:VALUE`. It then runs only that map's cases against the session, signing with the same values. Nothing else is passed through, so `-- --remote` never is.
- **Loopback only.** The URLs from the readiness file, and those a group run accepts through the internal `BYNK_TEST_DEV_URLS`, must be plain HTTP on `localhost`, `127.0.0.1` or `[::1]`. Anything else is refused, as a bare URL is.
- **Stopped by its own pid.** Each session gets SIGTERM to its own pid when its cases finish, so it stops its wranglers and their `workerd`s. After 30 seconds its process group (it leads its own) is killed, and nothing else is signalled.
- **The same refusals as against a deployment**, except that `env` is applied rather than checked against `vars`. Cron stays refused, because `/__scheduled` returns no result to check.

**Consequences.** `bynk-lang/bynk-dev` can become a thin wrapper over `--against dev`. A failing session leaves its log under the `--output` directory. The `--settle` retries (§3.6) come in a later slice; until then a remote `expect` isn't retried.
