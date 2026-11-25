# 0944 — `Agent(key).op.outcome(…)` returns a `CallOutcome`, as cross-context calls do

- **Status:** Accepted (v0.682)

**Context.** A failed agent call is a fault in the caller: a service that calls
`Basket(id).add(item)` cannot tell "busy, try again" (`AgentBusy`) from "the
handler faulted", cannot answer differently, and its `let … else` compensation
does not run. It is the agent half of what `.outcome` (#3894) does for a
cross-context call.

**Decision.** A. One outcome type for every call that might not reach its
handler: `CallOutcome[R]` from #3894, so an author learns it once, with the same
`o.handled()` for propagation. B. The mapping: `AgentBusy` (the call never
started) is `Unreachable(None)`; a fault in the handler, an invariant violation
and a rehydration violation (it ran, or its stored state did not load, and
nothing was committed) are `Faulted`; `ReentrantAgentCall` stays a fault, because
a call cycle is a bug and a value would invite retrying it. A failure to encode
the call or decode its answer is a toolchain bug and stays a fault too. `Rejected`
does not occur for an agent: there is no wire to refuse it. C. An invariant
violation is not its own variant: the agent invariants reference treats a
violation as a fault and a domain refusal as a `Result`, and a variant would
invite using invariants as validation. D. The form is grammar on the operation
reference, like #3894's, so it is legal wherever the plain call is, and an agent
handler may use it to call *other* agents; a call to `self` stays a plain call
(it is part of the same handler, ADR 0781). It is written for an inline
`Agent(key)` and for a local bound to an instance. E. Agent stubs at `unit` are
not part of this increment: the issue does not give the stub's address syntax,
and an agent call goes through the factory and registry, not `deps`, so the seam
needs its own interception point. It is recorded as a follow-up; the paths are
tested with real agents at `unit` (`Handled`, an invariant violation, a handler
fault) and in the runtime suite (`AgentBusy`, a rehydration violation, a
re-entrant call, across a Durable Object boundary).

**Consequences.** A service can compensate on every way an agent call fails. The
caller tells a busy instance from a faulting handler across a Durable Object
boundary by the kind its message begins with, since the instance's reply is a
bare `500`; #3932's marked `503` carries the same fact as a header, and the
recognition accepts both.
