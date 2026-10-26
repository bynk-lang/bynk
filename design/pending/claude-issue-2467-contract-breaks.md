---
level: minor
changelog: The editor lists every boundary change since `bynk.schema.lock` (events and agent state, compatible or breaking, under the checker's own rules) and every service contract that differs from `bynk.deploy.lock`'s live record, in **Bynk: Show Contract Diff** (`bynk/contractDiff`); the existing non-additive diagnostics are unchanged (#2467)
---

## ADR: contract-diff-reports-the-lock-classifier
title: The editor's contract diff reports the lock reconciliation's verdicts, and services are compared by hash only
summary: No second classifier, no duplicate diagnostic; service contracts are "changed", never "breaking"

**Context.** #2467 asked for two things: a warning, while editing, when a
boundary type change breaks `bynk.schema.lock` or a cross-context caller, and a
"Show Contract Diff" view. Against the code, the first is largely already done.
The editor reconciles events and agent state against the lock on every round
(#2081, #2204), through `reconcile` and `reconcile_agents` in `bynk-check`: the
same functions `bynk check`, the deploy build and the lock refresh run. They
report `bynk.event.non_additive_schema_change` and
`bynk.agent.non_additive_state_change` while you edit. Two parts of the
proposal had no counterpart in the code:

- The lock records only events and agent state shapes. Service contracts are
  not in it. A service's contract is a hash (ADR 0200, `own_contract_hashes`),
  and the only place a hash is compared with a record is the deploy gate
  against `bynk.deploy.lock` (ADR 0491). That comparison is equality: there is
  no compatible/breaking classification of a service contract anywhere.
- Within one source tree a caller and its callee compile together, so a
  cross-context caller cannot disagree with its callee until something is
  deployed.

`bynk.contract.*` is also already the namespace of the `requires`/`ensures`
clause diagnostics.

**Decision.**

- One definition of "breaking" (DECISION B). The contract diff is
  `diff_against_lock` in `bynk-check`, which runs the reconciliation's own
  classifiers (`reconcile_one` for events, `classify_agent_change` and
  `reconcile_agents` for agents) and reports their verdicts. An agent change a
  declared `migrate from N` step carries is `compatible`, as the build treats
  it. Nothing in `bynk-lsp` or `vscode-bynk` decides what breaks.
- No new diagnostic (DECISION A). The existing non-additive errors already fire
  while editing under the same debouncing as every project diagnostic, so a
  warning-grade sibling would report one change twice. The deploy gate and `bynk
  check` stay the authority. No `bynk.contract.*` code is added.
- `bynk/contractDiff` is a project-scoped custom request. Events and agents
  come from the round's lock diff. Services are each context's own contract
  hashes against the `contracts` record of its Worker in `bynk.deploy.lock`, per
  environment, read-only and per request, reported `changed`, `added` or
  `removed`. A Worker with no recorded contracts yields no row (silence is not a
  match, as in ADR 0491).
- Quick fixes are deferred (DECISION C).

**Consequences.** The view cannot disagree with `bynk check` about events and
agent state, and the lock, not the view, remains the one place that says what a
build accepts. Service contracts are described honestly: a changed hash means a
live caller would be refused at deploy, and the view does not claim more.

A real service-contract compatibility classifier is a separate piece of work. It
would be a function over two canonical contract forms
(`service_normal_form`, ADR 0200: the `old` form recorded and the `new` form
built) returning compatible or breaking by rules such as "a request field added
with a default, or a response field added, is compatible; anything removed,
retyped or made required is not". It would live beside
`service_contract_hash` in `bynk-check/src/contract.rs`, so the checker, the
deploy gate and the editor share it. For the gate to use it, the ledger would
have to record each Worker's normal form, not only its hash (an additive,
`default`ed field, like `expects`), and `deploy --context` would then accept a
compatible change and keep refusing a breaking one under
`bynk.deploy.contract_skew`. The runtime hash check would also need to accept a
compatible older hash. That changes what a pushed Worker answers with in
production, so it is out of scope for an editor increment and wants its own
proposal.

**Tooling delta.** Hover, completion, semantic tokens and signature help are
unchanged, because the view adds no language surface. A "locked at vN" hover
note on boundary types was left out: the diff view carries the locked version.
