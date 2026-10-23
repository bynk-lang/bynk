---
level: patch
changelog: "`bynk test` reports which agent `invariant` or `transition` refused a failed case as an optional `violation` field (`kind`, `agent`, `name`), in the JSON document and on the `ndjson` case event (Test Explorer depth track, Slice F)"
---

## ADR: test-json-violation-field
title: A case refused by an agent invariant or transition reports which one as data
summary: An optional `violation` after `counterexample`; the generated check says whether it was an invariant or a transition, because one name may be both

**Context.** A case that made an agent refuse a commit failed with `Error: InvariantViolation: Wallet.nonneg` and nothing else (ADR 0107). The message names the agent and the declaration but not whether the declaration is an `invariant` or a `transition`, and the checker lets one agent declare one of each under a single name, so `Agent.name` does not identify the declaration: the two fail with identical text. A tool wanting to show or open the declaration had only that string to go on, and parsing it would freeze the runner's wording as a contract. ADR 0572 fixed the order in which optional `Case` fields are appended.

**Decision.** `Case` gains an optional `violation` after `counterexample`, so after `tier`, `kind`, `seed` and `counterexample`, and each is still omitted when absent. It is an object `{ kind, agent, name }`: `kind` is `invariant` or `transition`, `agent` the agent's name, `name` the declaration's. The `ndjson` `case` event carries it with the rest of the case, ahead of `durationMs` (ADR 0583), which stays last.

The kind comes from the generated check, not from the message, because only the check knows it. The runtime helper `invariantViolation(agent, name)` takes an optional third argument, `"invariant"` or `"transition"`, which it stores on the fault's `invariantViolation` record as `declaration`; the two generated checks pass it. The argument is optional so that a caller which does not pass it, and a fault from an older runtime, still throw as before and simply report no `violation`. The fault's message, its classification as an uncaught fault (ADR 0107) and the commit refusal itself are unchanged.

The test runner reports it from the two places a refusal is caught. A plain `case` reads it from the fault it caught; a `property` reads it after shrinking, from the fault the shrunk input provoked, because a refusal is now a shrinkable failure and a shrunk property reports a counterexample and a seed like any other. A refusal inside a driven history is not thrown to the property: it is recorded on the step as `accepted: false` (ADR 0155 D4), so a history reports no `violation`, and attributing a refused step to its declaration is left for a later increment.

Unlike the generative fields of ADR 0572, `violation` is not gated on the build having a generative module, since a plain `case` can be refused as well as a property. The runner returns it beside `error`, and a run in which no case was refused prints the same document and stream as before.

**Consequences.** A consumer can tell a refused invariant from a refused transition, including when they share a name, and must not assume that the message says so. A consumer must ignore a `violation` whose fields it does not understand, as with any optional field. The declaration's source position, the call that provoked the refusal and the rejected state are not reported; a position can be added to `violation` later without changing its order, and the rest belongs to a separate proposal. The `@bynk/runtime` and `@bynk/wire-client` helpers gain the same optional argument, being generated from one source.
