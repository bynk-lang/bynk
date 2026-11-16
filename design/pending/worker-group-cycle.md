---
level: patch
changelog: Two `[deploy.workers]` groups whose members bind to each other in both directions are refused (`bynk.deploy.group_cycle`), even though the contexts themselves are acyclic (#3562, G2b)
probes: diagnostic_coverage, emit_diagnostics, explained_codes
---

A Worker is uploaded after the Workers it binds to (ADR 0193), and the language
rules out a cycle among *contexts* (`bynk.context.consumes_cycle`). A group folds
its members into one Worker, so two groups can depend on each other with no cycle
among their contexts: `A1` calls `B1` and `B2` calls `A2`, with `A1` and `A2` in
one group and `B1` and `B2` in another. The check reads the Service Bindings the
Workers would declare, a context standing for its group's Worker, and reports one
error per cycle through a group, naming the Workers and one context edge per hop.
A call inside a group is a Worker binding to itself and is no edge, and a cycle
among ungrouped contexts is not this check's. It runs at check time, so a project
learns of it before any deploy, and ADR 0193's conclusion that no two-pass deploy
is needed holds for groups.
