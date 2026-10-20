---
level: patch
changelog: The agent state view no longer flags a move a `transition` would refuse when the agent can only make that move on its first commit, which the runtime never checks
---

## ADR: transition-overlay-genesis-commit
title: The `transition` overlay checks only moves from states the agent can be in after a commit
summary: The genesis commit runs no `transition`, so a move out of the initial state is flagged only when some committing path can leave the agent there again

**Context.** A `transition` is checked from the second commit onward: the genesis commit, an agent's first, has no `old` and is skipped (`book/reference/agent-invariants.md`). The overlay of [ADR 0539](0539-transition-overlay-is-a-view-annotation.md) checked every move from a reachable start state, so a violating move out of the initial state was flagged even when the only way to take it was the agent's first call, which never faults. Slice 0 of the transition-diagnostics track (#2215, proposal #2219, track §3.9) fixes that before the same model feeds a checker warning.

**Decision.** The state model computes, per state field, `reachable_after_commit`: the states the agent can be in after at least one commit. Every reachable state other than the initial one is in it, because reaching it took a write and a write commits. The initial state is in it only when some committing path from a reachable start leaves the agent there: an unchanged path from the initial state in a handler that commits, a move into it, or a computed write, which can land anywhere. Without a literal initialiser every reachable state counts. The overlay checks only start states in this set; `reachable` is unchanged and still drives the view's dimming, since the agent really is in its initial state before the first commit. The set is sent on the wire as `reachableAfterCommit`. This amends ADR 0539's "only reachable start states count".

**Consequences.** A one-way move out of the initial state that a `transition` refuses is no longer flagged, matching the runtime. A refused call from the initial state in a handler that commits is still flagged, because its genesis call is skipped but the second call faults. The slice-2 warning (#2221) reads the same model, so it inherits the rule.
