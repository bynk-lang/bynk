---
level: patch
changelog: The agent state view shows each handler's effects — what it writes, reads and calls on each path, and what it yields — in place of a state machine for an agent with no enum state field, and collapsed under the machine for the rest
---

## ADR: agent-effect-summary
title: The agent effect summary is a second, state-free walker sharing the state model's conventions, merged on writes, reads and calls
summary: One row per merged path; writes via the shared store-write recogniser; reads exclude fields the path already wrote; same budget

**Context.** Slice 4 of the agent-state-views track (#2190, proposal #2198). Most agents in `examples/` hold maps, counters and logs, not an enum state field, so the state diagram has nothing for them. The proposal (DECISION A) asked that the summary reuse slice 1's path pass with a different merge key, so the two could not drift.

**Decision.** `bynk_ide::agent_effects::agent_effects` walks each handler with the state model's conventions — the same statement-level scope (ADR 0259), the shared store-write recogniser (`store_write`), joins that merge paths producing the same row, guards combined the same way (a condition and its negation now cancel to none, in both views), and the same 256-path budget — but as a separate walker, because it tracks no state field and needs none of the state model's narrowing; sharing the state walker would have meant threading an empty field through every narrowing branch. A row is a merged path: its guards; its writes in order (`:=`, or the store operation, with the key or item when it is a plain name or literal); the store fields it only reads, any bare-name, query-accessor or read-operation reference not shadowed and not already written on that path; the agents it calls (`Agent(key).method`, against the context's agent table, as the sequence diagram classifies them); and its outcomes. Paths merge when their writes, reads and calls agree; outcomes are collected. The server sends the effects with the state model; the view shows them in place of the empty state for an agent with no state field, and collapsed under the machine otherwise, highlighting the handler under the cursor.

**Consequences.** Every agent in `examples/` gets a useful view. The second walker is small and shares the recogniser, scope and join rules with the state model, so the two agree on what a write is; the narrowing that would differ is exactly what this walker doesn't need. A write inside a lambda or an argument is out of scope, as everywhere in these views. Calls into a consumed context are not listed (the sequence diagram is the view for boundaries).
