---
level: patch
changelog: A cycle between Workers that involves a `[deploy.workers]` group is refused at check time as `bynk.deploy.worker_cycle` (renamed from `bynk.deploy.group_cycle`, never released), whichever two Workers and however the cycle is closed: a call, or a publisher's binding to its subscriber (#3562, G2b)
probes: diagnostic_coverage, emit_diagnostics, explained_codes
---

A Worker is uploaded after the Workers it binds to (ADR 0193), and the language
rules out a cycle among *contexts* (`bynk.context.consumes_cycle`). A group folds
its members into one Worker, so a group and any other Worker can depend on each
other with no cycle among their contexts. This is the check's rename from
`bynk.deploy.group_cycle`, and the cases it now covers beyond the first version: a
cycle through three or more Workers, and one closed by a publisher's binding to its
subscriber (the events fan-out) rather than a call. A cycle among Workers of
contexts in no group is not this check's: a subscriber that also calls its publisher
runs under `bynk dev` and in tests, and `bynk deploy` already refuses it before
uploading anything, since Cloudflare rejects an upload whose binding target does
not exist yet (code 10143). The bindings this reads are built only for a project
with no error, so a `consumes` cycle is reported first and a Worker cycle once it is
fixed: never the same cycle twice.
