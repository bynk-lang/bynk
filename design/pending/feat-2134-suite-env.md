---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: A suite or case can give its participants variables and secrets with `env { NAME = "value" … }`. At `system` the entries go on each participant's Worker `env`; at `unit`/`integration` they are what the `Secrets` double answers. A case's `env` layers over its suite's, and an entry naming a `Bearer` actor's secret is the one the `system` driver signs with. New diagnostics `bynk.suite.duplicate_env`, `bynk.suite.env_not_literal` and `bynk.suite.env_binding_clash` (#2134)
---

## ADR: suite-env-configuration
title: A suite gives its participants variables and secrets with `env { … }`
summary: Test configuration lives in source, one map for every participant, at every tier

**Context.** A `system` suite could not say what a Worker's variables are.
`bynk-lang/shop` chooses its payment processor with `PAYMENTS_PROCESSOR=fake`,
which could only be passed to `bynk dev` with `--var`, and `stub Secrets.get(…)`
is refused at `system` (`bynk.stub.at_system`) because each Worker builds its
own providers. So the shop's flow could not run at `system` with the fake
processor. The harness `env` held only bindings and call keys, the Bearer test
secret was a fixed `"__bynk_test_secret"` in `process.env`, and the `Secrets`
test double answered `None` for every name (end-to-end-testing track §3.1,
proposal #2134).

**Decision.**

- **Syntax (D-E).** An `env { NAME = "value" … }` clause is a member of a suite
  body and leads a case body, beside any `stub` clauses. `env` is contextual: a
  keyword only there, and only when `{` follows it. Entries are separated by
  newlines or commas.
- **Values.** A value is a `String` literal, matching what a Worker's `env`
  holds (`bynk.suite.env_not_literal`). A name given twice at one scope is
  `bynk.suite.duplicate_env`.
- **One map for every participant (D-A).** Per-context maps
  (`env shop.payments { … }`) are re-openable on the first project where two
  contexts need different values under one name.
- **A case layers over its suite**, name by name, like `stub` precedence. A
  suite's entries apply to its own cases; a `property` runs with none.
- **Every tier (D-B).** At `system` the running case's entries are put on each
  participant's Worker `env`, before the harness's own call keys and bindings,
  and a name a participant already binds, or a key the harness sets there itself (a call-edge key, a `Foreign` caller's secret), is `bynk.suite.env_binding_clash`. At
  `unit`/`integration` they are what the `Secrets` double answers
  (`Some(value)`; other names stay `None`). It is not a `stub`: it sets the
  environment and replaces no seam, which is why it is legal at `system`.
- **Test secrets (D-C).** An entry naming a `Bearer` actor's declared secret
  wins over the fixed test secret: the `system` driver signs with it, and the
  seam, which reads the Worker's `env` first, verifies with it.
- **Source, not manifest (D-D).** No `[test.env]` table in `bynk.toml`; if a
  remote run (slice 5) needs a project-wide default, that re-settles §3.1.

**Mechanism.** Every `system` driver builds its own harness, with no case in
hand, so the running case's map is a module-level `__bynkEnv` that each case
runner sets first; `__makeHarness()` copies it onto each participant's `env`,
and a Bearer driver reads its secret from it before `process.env`. The unit
module does the same for its `Secrets` double, clearing the map when the case
finishes. All of it is emitted only when some case in the module has entries,
so a suite without `env` emits exactly as before. Production emit is untouched
(ADR 0147).

**Consequences.** The shop's fake-processor flow can run at `system`, and one
case body keeps its configuration as it moves between tiers. The binding-clash
check needs the Worker bindings, which the emitter computes, so it runs on an
otherwise clean build and not in the language server's check pass.
