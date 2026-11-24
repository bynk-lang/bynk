---
level: minor
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: An agent call that waits past its queue limit (`AgentBusy`) now answers `503` with `Retry-After: 1` at an HTTP route, logged as busy rather than reported as a fault; `@busyAfter(d)` before `agent` lowers the 30-second wait
breaking: An HTTP route whose agent call hits `AgentBusy` now answers `503` with `Retry-After: 1` instead of `500`, and is no longer reported to the platform as a fault, so a client or alert that matched the `500` must match the `503`; a cross-context call that does so is `503` too, unmarked, so its caller reads it as unreachable rather than `CalleeFaulted`.
---

## ADR: agent-busy-503
title: `AgentBusy` answers `503` with `Retry-After`, and `@busyAfter(d)` lowers the 30-second wait
summary: A busy agent instance is load, not a fault: an HTTP route answers 503, and an agent may lower its queue wait

**Context.** A call that waits 30 seconds behind a hot instance's queue faults
`AgentBusy` (ADR 0781). That is a transient condition, but at an HTTP handler it
took the ordinary fault path: `500`, logged and reported as a fault. Clients
were not told to retry, operators saw fault alerts for load, and the 30-second
limit could not be lowered, so a latency-sensitive route could not fail fast.

**Decision.** A. Only `AgentBusy` is mapped; `ReentrantAgentCall` stays a fault
and a `500`, because it is a bug, not load. B. An `AgentBusy` that reaches the
Worker entry's fault handler answers `503 Service Unavailable` with
`Retry-After: 1`, a short fixed hint (queue waits are seconds, and clients should
back off themselves). It is logged once, where the queue gave up, as
`AgentBusy <Agent>` with the queue depth, and is not reported through
`waitUntil` as a fault. The body is plain text until the service error format
(#3916) lands. The Durable Object marks its own `503` (`x-bynk-agent-busy`,
naming the agent) so the calling Worker reads it back as `AgentBusy` rather than
as a bare failure. C. `@busyAfter(d)` before `agent` lowers, never raises, the
wait: a `Duration` literal from 1 second to 30 seconds, otherwise
`bynk.agent.busy_after_out_of_range`; it is per agent class, and any other
agent annotation, a repeat or a wrong argument is `bynk.agent.bad_annotation`.
The emitted module records the wait on the class (`__agentBusyAfter`) and the
runtime reads it where the class is wrapped, so the bundle target, the
integration tier and Workers share one source of truth. D. Elsewhere: in a queue
consumer `AgentBusy` is a fault, which retries the message; in a cron run, a
fault; in an event subscriber, a fault (it becomes a retry under the
durable-delivery track, #3898), so the entry leaves an `event <service>` route
alone; in a cross-context call it is `503`, unmarked, which a caller's
`.outcome(…)` reads as `Unreachable(Some(503))` (#3894).

**Consequences.** A hot instance shows up as `503` and in the busy log, not as
fault alerts, and a route can fail fast. A client that matched the `500` for a
busy agent must match the `503`: recorded as breaking. The tree-sitter grammar
admits `@…` before `agent`, with conflicts declared on the two type forms and
the derived capability, whose own trailing annotations it shares a token with.
