# 0487 — An agent handler cannot `~>` send; a store operation is refused with its own code

- **Status:** Accepted (v0.342). **Amended by [[0824]]** (the refusal moves to `detach`, with `bynk.detach.in_agent_handler` and `bynk.detach.store_op`; `~>` shares it while deprecated).

**Context.** A `~>` send lowers to `deps.__exec.waitUntil(…)`, and `__exec` is
threaded only into service handlers, from the Worker's request context (ADR
0106). An agent handler is a Durable Object method: on the workers target its
`deps` are rebuilt in the object's `fetch` from the handler's own `given` and
`env`, and on the bundle target it receives its caller's `deps`. Neither carries
an `__exec`, so every `~>` in an agent handler (a store operation, a capability
call, another agent's handler, a send inside a closure) passed `bynkc check` and
then failed `tsc` on both targets (#2017). No program using one has ever
compiled.

**Decision.** The checker refuses a `~>` anywhere in an agent handler body,
closures included. A send whose operation is one of the agent's own store
operations gets `bynk.send.store_op`: a store write is staged and committed
atomically at the handler boundary, so it is not a message to anyone and there is
nothing to send without waiting for; the note suggests `do field.op(…)` or
`let _ <- field.op(…)`. Any other send gets `bynk.send.in_agent_handler`, whose
note suggests awaiting the effect or sending it from the calling service handler.
When either fires, the reply-shape checks (`bynk.send.requires_unit`,
`bynk.send.non_effect`) are skipped, since their remedy no longer applies.

Wiring an `__exec` for agents was the alternative. It would have to be supplied
in three places that share nothing today: the Durable Object's `fetch` (from its
state's `waitUntil`, which the runtime's `DurableObjectState` does not declare),
the bundle path, where an agent method runs on its caller's `deps` and so would
see an `__exec` only when the caller happens to send, and the test harness's
in-memory namespaces. That is a feature with its own semantics to settle (what a
send outlives in an object that is not torn down with a response), not a fix.

**Consequences.** Every program this rejects already failed to build, so nothing
that worked breaks. An agent awaits its effects. Fire-and-forget from inside an
agent can return later as its own increment, by giving agent handlers an
execution context and lifting `bynk.send.in_agent_handler`; `bynk.send.store_op`
stays either way.
