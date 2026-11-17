---
level: minor
changelog: A call between two members of a `[deploy.workers]` group stays inside the Worker instead of crossing a Service Binding to itself; it is the same signed, contract-checked, admitted request, so values are copied, a callee's fault is the same `CalleeFaulted` and a refusal the same refusal (#3562, G3-2)
probes: ts_writes
---

A call between two members of a group is a call from the Worker to itself. The group's
entry now answers the binding a member names another by, in process: the request runs
through the group's own dispatcher (the same one a request from outside takes) with no
Service Binding hop. It is deliberately not a direct function call. The request is
still signed under the edge's key, still carries the contract hash, is still
admission-checked by the callee's entry and decoded on both sides, so a value is
copied across it (neither side can observe the other mutating one), a callee's fault
reaches the caller as the same `CalleeFaulted` with the same redaction, a callee
that does not admit the caller refuses it the same way, and a caller with no key for
the edge cannot make the call even in process. Only the network hop and the extra
subrequest are gone. A `scheduled`, `queue` or `email` handler reaches a member the
same way.

The events fan-out is a Durable Object, a different isolate from the Worker, so it
cannot call back into the Worker's dispatcher and keeps delivering to a member
subscriber over the Worker's own Service Binding. That binding therefore stays in the
group's `wrangler.toml` for now (G3-3).
