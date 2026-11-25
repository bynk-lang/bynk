---
level: minor
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: A test stubs an agent operation at `unit`: `stub Basket.add(_) faulted` or `unreachable` makes the calls it matches, whatever the key, end as `Faulted` or `Unreachable(None)` in `Basket(k).add.outcome(x)`, and fault a plain call; only an agent of the context under test, only at `unit`, and only those two words (`bynk.stub.agent_stub`)
---

## ADR: agent-operation-stubs
title: An agent operation is stubbed at `unit`, by intercepting the agent's own calls
summary: The address, scope, words and interception point of `stub <Agent>.<op>(…) faulted | unreachable`

**Context.** #3933 made `Agent(key).op.outcome(…)` return a `CallOutcome` and left a
test no way to drive its `Unreachable` (a busy agent) or `Faulted` arms. Its
Decision E said an agent operation could be stubbed at `unit`, "extending #3892's
seam to agents", which is not a simple extension: #3892's service stubs overlay
`deps.surface`, which handlers receive, while an agent call goes through the
emitted `__make<Agent>` factory and the agent's registry, never through `deps`.
The issue (#4215) settled the four decisions below, all as recommended.

**Decision.** A. The address matches every instance:
`stub <Agent>.<op>(<arg patterns>) faulted | unreachable`. A key-specific form can
be added later without breaking anything. B. Same-context agents only. Another
context's agents are reached through its services, which `stub` already covers; a
stub naming one is `bynk.stub.agent_stub`, where any other unknown name stays
`bynk.stub.not_a_seam`. C. `faulted` and `unreachable` only. A stubbed
`returns <value>` would bypass the agent's state and invariants, which a unit test
reaches through the real agent; any other word is `bynk.stub.agent_stub`.
D. Intercepted at `unit` only. The `integration` tier runs the real agents, so an
agent stub in an `integration` case or suite is `bynk.stub.agent_stub` (at
`system`, `stub` is already `bynk.stub.at_system`). The clauses resolve into a
`ResolvedStub` marked `agent`, with a synthetic capability whose operations are the
stubbed handlers (as #3892 does for a consumed service), so the argument patterns,
the scoping (case, suite) and the fixtures lower through the existing
`__Stub_<Agent>` class. That class is not a `deps` entry: the unit tier's deps
factory, which every case and property builds its `deps` with, registers the
classes with the runtime (`__stubAgents`), and the bundle-agent wrapper asks the
class before it runs an operation. A matched clause throws an error marked with the
outcome it stands for and the agent it is for, which `callAgentOutcome` reads back
as that outcome, for the called agent only: an unreachable stub of another agent,
met inside a handler that ran, is that handler's fault, as a nested `AgentBusy` is.
A call no clause matches throws a `bynkNoStub` error, and the real operation runs.
A registry reset ends the stubs with the case.

**Consequences.** Production code and the other tiers are unchanged: the table is
empty unless a unit suite stubs an agent, and the emitted agent modules and the
Worker entries do not change. An agent stub is by operation, never by key, so a
test cannot make one instance busy and leave another working; a key-specific form is
the extension if it is wanted. The runtime's bundle-agent wrapper awaits the
stub check only while a stub is registered, so with none (production, and any suite
that stubs no agent) a call still enqueues synchronously, at call time, in the order
it was made, as before; a call made while a stub is registered waits one microtask
first, which can reorder it against an unstubbed call made in the same tick.
