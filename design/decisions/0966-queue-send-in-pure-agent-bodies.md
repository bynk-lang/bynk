# 0966 — A queue send in a transition or migrate body is refused, with a note naming the capability

- **Status:** Accepted (v0.702.1)

**Context.** The producer check (#3879) walks functions, service and agent handlers and provider operations, and not an agent's `transition` predicates or `migrate` steps. Whether a send there is legal decides whether the walk must widen.

**Decision.** A send is not legal there. A transition predicate and a `migrate` step are pure (`bynk.transition.impure_predicate`, `bynk.agent.migrate_impure`), and a capability is not in scope in them, so `OrderEvents.sendValue(…)` is already `bynk.resolve.unknown_name`. The producer walk therefore stays as it is. What changes is the diagnostic: when the unknown name is a capability the context declares, it carries a note saying so and where a capability can be called. A negative fixture covers a send in a transition and one in a `migrate` step.

**Consequences.** No new diagnostic code and no change to what compiles. An unknown name that is a capability reads differently, in any pure position (a transition, a `migrate` step, an invariant, a contract), and for a capability consumed from another unit as well as one declared here. Hover, completion, semantic tokens and signature help are unchanged.
