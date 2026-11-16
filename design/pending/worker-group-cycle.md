---
level: patch
changelog: A `[deploy.workers]` group and another Worker (another group or a single context) whose members bind to each other in both directions are refused (`bynk.deploy.worker_cycle`), even though the contexts themselves are acyclic (#3562, G2b)
probes: diagnostic_coverage, emit_diagnostics, explained_codes
---

A Worker is uploaded after the Workers it binds to (ADR 0193), and the language
rules out a cycle among *contexts* (`bynk.context.consumes_cycle`). A group folds
its members into one Worker, so a group and any other Worker can depend on each
other with no cycle among their contexts: `A1` calls `B1` and `B2` calls `A2`, with
`A1` and `A2` in one group and `B1` and `B2` in another, or `A1` calls `C` and `C`
calls `A2`, with `C` a Worker of its own. The check reads the Service Bindings the
Workers would declare, a context standing for its group's Worker, and reports one
error per strongly connected set of Workers containing a group, showing one cycle
with a context edge per hop. A call inside a group is a Worker binding to itself and
is no edge. A cycle among ungrouped Workers is not this check's: `consumes_cycle`
rules out service-call cycles, but a publisher-to-subscriber binding can still close
one with a call in the other direction, a pre-existing gap left for a follow-up. It runs at check time, so a project
learns of it before any deploy, and ADR 0193's conclusion that no two-pass deploy
is needed holds for groups.
