# 1064 — The editor's Analytics surfaces read the checked mapping, and the call sites from the text

- **Status:** Accepted (v0.796)

**Context.** #4502 is the tooling delta of #4105 and #4451 that of #4104. The mapping a written record gets lives in the checker's `Callee::AnalyticsWrite` records, which the language server's round did not expose, and the lock's check ran only in the driver.

**Decision.** `ProjectAnalysis` carries the live mapping (`analytics`, keyed `<unit>.<Instance>`) and reconciles it against the lock it read, scratch-copied so the editor never writes the lock; its diagnostics join the round's. Hover on a record field appends the column through that table. The call-site features (instances in the first argument, a select's read-type fields, `{dataset}` hover and tokens) find the `AnalyticsSql.query` / `select` calls in the live buffer, as the other argument-position completions do, so they work while the buffer does not check. `@index` and `@name` on a record field are tokenised as annotations.

**Consequences.** The editor and `bynk check` agree on the lock diagnostic. The dataset table named by the hover is derived from the context and instance names, mirrors the emitter's `analytics_dataset_name` (the context's dashed name, then the capability in snake case; a project that groups contexts into one Worker names the group instead), and says only that the environment suffix follows `--env`.
