---
level: patch
changelog: Any cycle between Workers is refused at check time as `bynk.deploy.worker_cycle` (renamed from `bynk.deploy.group_cycle`, which never shipped), including one through the events fan-out among contexts in no group (#3562, G2b)
probes: diagnostic_coverage, emit_diagnostics, explained_codes
---

A Worker is uploaded after the Workers it binds to (ADR 0193), so a cycle between
Workers cannot be deployed. The check read only cycles through a `[deploy.workers]`
group; it now reads every cycle in the Worker-level graph. A publisher's Worker
binds to each subscriber, opposite to the subscriber's `consumes`, so a context that
subscribes to an event and also calls its publisher closes a cycle with no group
involved. The message names a group only when one is in the cycle. A cycle among
contexts' `consumes` stays `bynk.context.consumes_cycle`'s. The bindings this reads
are built only for a project with no error, so that cycle is reported first and a
Worker cycle once it is fixed: never the same cycle twice.
