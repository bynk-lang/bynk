---
level: patch
changelog: The IDE crate derives each agent's state machine from its handlers (`bynk_ide::agent_state::agent_state_model`) — the model the coming state view renders; no user-visible change yet
---

## ADR: agent-state-model-data-source
title: The agent state model is a dataflow pass over a re-parse, placed in `bynk-ide` as a pure function of the agent and the type table
summary: No retained checker callees; merge at joins with a 256-path budget; reachability over-approximates; movable below `bynk-ide` later

**Context.** Slice 1 of the agent-state-views track (#2190, proposal #2195) needs, for every `store Cell` of a user sum type, the edges an agent's handlers can produce and the states reachable from the initialiser. The settled track (§3.3) chose a re-parse over retaining the checker's resolved `Callee` table in `ProjectAnalysis`, as ADR 0301 did for the architecture map, and a path pass bounded for a live view (ADR 0504). §3.7 asked that the model be reusable by a future declared transition table, a language increment the Book's agents reference defers.

**Decision.** `bynk_ide::agent_state::agent_state_model(&AgentDecl, &HashMap<String, Arc<TypeDecl>>)` is a pure function of the agent's AST and the unit's combined type table (`ContextBoundaryInfo::types`), taking no LSP or checker-internal type. A state field is a `Cell` whose type resolves to a user sum in that table; payloads abstract to constructors, and a field whose type doesn't resolve is skipped. Each handler is walked as a dataflow pass over abstract paths `(from-set, current value, last write, guards, outcome)`:
- `match`/`is`/`==`/`!=` on the field (with `!`, `&&`, `||` over such tests) narrow the from-set before a write, are decided statically after a known write (a read sees the staged value, ADR 0125 D4), and become guards after a computed write. Any other condition is an opaque guard, labelled with its source text. The narrowing is local to this module rather than `bynk_check::narrowing::matched_is_tests`, because it needs negation (an `else` removes states) and state-set results, which that function, built for positive `is` facts, doesn't give.
- At each join, paths that agree on from-set, current value, last write and outcome merge; differing guards combine into one disjunction.
- The last write wins; a domain `Err` after a write is still an edge; fault paths are not modelled.
- Past 256 live paths a handler is truncated to one `Unknown` edge from every state.
- Reachability runs from the initialiser (every state when it isn't a variant literal), with an `Unknown` edge reaching every state.

The store-operation recogniser slice 0 added to the sequence diagram moves to a shared `bynk-ide` module, so the two views agree on what a write is; the state model reads it for `Cell.update` on the state field.

**Consequences.** No `ProjectAnalysis` change and no new checker table. The model is sound in the sense the view and the coming `transition` overlay need: a state outside `reachable` is truly unreachable, and a truncated handler loses precision, never edges. Because the function depends only on the AST and a type table, a future declared-transition-table rule can move it into `bynk-check` and compare the declared relation with the derived one without rewriting it. Guards are text, not a predicate IR; a consumer that needs to evaluate them (none does yet) would need more than this model carries.
