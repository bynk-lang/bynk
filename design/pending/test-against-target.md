---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: `bynk test --against <name>` runs the `system` suites, unchanged, against a deployment named under `[test.targets]` in `bynk.toml`. Each Worker's URL comes from the target, and its secrets come from the environment. A bare URL, an unknown name, a `production = true` target and a missing secret are refused before any request. A case a real runtime can't honour, or whose `env` disagrees with the target's `vars`, is refused with a `bynk.test.remote_unsupported_*` diagnostic, or skipped under `--skip-unsupported`. `--format json` gains `transport`, plus `skipped` when a case is skipped (#2138)
---

## ADR: end-to-end-testing-is-the-languages
title: End-to-end testing against a running system is the language's, as a run of the `system` suites
summary: Supersedes ADR 0207's "e2e is external" line for running against a deployment; the credential ceremony with a real IdP stays outside

**Context.** ADR 0207 placed the top rung, a deployed instance, outside the language ("e2e (Postman, external)"). So every project hand-rolled it in another language, with hand-signed tokens, untyped string matching and port guessing. `bynk-lang/shop` keeps a Bash scenario script for exactly this. The `system` drivers already build a concrete `Request` for a route and decode its `Response`; only the hop between them was in process. (End-to-end testing track §3.0, #2138.)

**Decision.** Running the `system` suites against a running system is the language's. The same case bodies run against a deployment by a choice of the run, `bynk test --against`, without rewriting them elsewhere. What stays external is what was always the point of "proper auth": a real identity provider's credential ceremony.

**Consequences.** ADR 0207's "e2e is external" line is superseded for running against a deployed instance. The in-process `system` tier stays the default and the gate.

## ADR: test-against-a-named-target
title: `bynk test --against <name>` runs the `system` suites against a named deployment, refusing what it can't honour
summary: A runner flag, not a tier; named targets only, production refused, secrets from the environment; remote-unsupported cases refused or explicitly skipped

**Context.** Track §3.5, §3.7 and §3.8 (#2138, DECISIONS A, B and E) settle how a run against a real runtime is asked for, kept safe, and limited to what it can honour.

**Decision.**

- **A runner flag, not a tier (A).** Where the Workers run is a property of the run, like `--target`, so it is `--against`, not an `as e2e` tier that would put the same body in two places.
- **Only named targets (E).** `--against` takes `dev` (reserved for the local `bynk dev` the runner starts, a later slice) or a name under `[test.targets]`, never a URL. A target marked `production = true` is refused. A target lists the secrets its Workers verify with, by name, and the runner reads each from the environment, never from `bynk.toml`. A name that isn't set refuses the run. The target and each Worker's URL are printed before the first request.
- **The transport is the harness, compiled for the run.** A remote build emits a harness whose bindings send each driver's request to the target's URL (`BYNK_TEST_TARGETS`, keyed by worker name). The driver bodies are unchanged, and the in-process build's output stays byte-identical. The fixed test secrets aren't assigned, so the drivers sign with the environment's.
- **Only the `system` suites run.** Unit and `integration` suites stay in process.
- **What a real runtime can't honour is refused (B).** A deployment answers only on its public `http` routes. A remote build therefore refuses a `system` case that addresses an `on call` service (internal, and signed per pair) or uses `advance Clock` (the deployed Workers read wall-clock time). The issue's list omits two more cases, which follow from "only public surfaces remain" and are refused too: a queue `message` and a cron `schedule` against a named target, because no public wire reaches a queue consumer or a cron handler, and an asset fetch, which is answered in process. The track's other two refusals, a direct agent read and `svc.event(…)`, need no remote check, because the `system` tier itself already refuses them: an agent isn't in a `system` case's scope, and an event address is `bynk.test.event_needs_in_process`. Each refusal is a `bynk.test.remote_unsupported_*` diagnostic naming the case. `--skip-unsupported` skips such cases instead, naming each one and why (`outcome: "skipped"` in `--format json`). Without it, a suite can't silently shrink to nothing.
- **A case's `env` must agree with the target's `vars` (§3.7).** A deployed target already runs with its own variables, so the runner can't apply a suite's `env`. A case runs only if its `env` and the target's declared `vars` agree on every name either mentions. It is refused (`bynk.test.remote_unsupported_env`) when it sets one of the target's secrets, sets a name the target doesn't declare, sets a different value, or leaves unset a name the target declares (in process an unset name reads as `None`, so that would be a different environment).
- **Reporting.** `--format json` gains `transport: "remote:<name>"`, plus `skipped` when a case is skipped. Both are absent from an in-process document, so its layout is unchanged.

**Consequences.** Fresh state per case isn't promised remotely, because a deployment keeps its state between runs; `run.id` (§3.7) is a later slice. `--against dev` and the `--settle` retries (§3.5, §3.6) come in later slices.
