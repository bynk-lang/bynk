---
level: patch
changelog: A queue step stopped part-way whose record then fails to write (#3999) reports both failures, the one that stopped it first, instead of only the write's
---

## ADR: a-stopped-step-reports-both-failures
title: A step stopped part-way whose record also fails reports both failures
summary: when `apply_kind` cannot write the commits of a step that stopped part-way, it reports the stopping failure and then the write's, keeping the stopping failure's exit code

**Context.** #3999 (#4612) let a resource step stop part-way and still record what it did: `apply_kind` writes the step's commits, then returns its failure. Its review found that when one of those writes fails too, the write's error was returned and the failure that stopped the step was lost, which makes an incident hard to follow.

**Decision.** If a stopped step's commit fails to write, `apply_kind` returns one failure whose message is the stopping failure's, then `; and then `, then the write's, with the stopping failure's exit code. A step that did not stop reports a failed write alone, as before.

**Consequences.** Neither cause is lost. `kind/queue.rs::a_stopped_step_whose_record_fails_reports_both_failures` pins the order, and fails without the change.
