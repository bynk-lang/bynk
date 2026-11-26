---
level: patch
changelog: A `PartialContent` or `NotModified` that a helper `fn` returns from a non-GET route is a logged fault that names the status and the route
---

## ADR: partial-content-off-get-is-a-runtime-fault
title: A 206 or 304 returned to a non-GET route through a helper is a run-time fault with a named log line, not a compile-time check
summary: Why the helper case is not analysed statically, and what the fault logs

**Context.** #4467, a follow-up to #4389. `PartialContent` and `NotModified` are refused at compile time on a non-GET route when its own handler body names one (`bynk.http.status_needs_get`). One built in a helper `fn` and returned from a POST route was caught only at run time, as a `500` whose log line did not say which route.

**Decision.** The run-time check stays the rule for the helper case, and its log line is made specific: `PartialContent (206) is a GET answer, but POST /orders returned it: a range or conditional read is only served by a GET route, so build it in a GET handler (a helper fn cannot be checked at compile time)`. The dispatch for a non-GET route passes its own method and path to the response conversion, so the message names the route to fix. The direct case keeps its compile-time refusal. A static analysis through helpers was rejected: an `HttpResult` is an ordinary value, so it reaches a route's response through `let`, `match` arms, function arguments, lists and records. A precise check would need a flow analysis of which variants every expression can produce. A conservative per-function summary ("may produce 206 or 304") would refuse a helper that builds one only on a branch a POST route never takes, or one that a POST route calls and discards, which is valid. The run-time fault is exact, since it fires only when the value reaches the response, and it never sends the wrong status.

**Consequences.** Behaviour is unchanged (still `500`, never sent); only the logged message and the generated `notGet` option (now the route, not `true`) change. Generated Worker entries for non-GET routes differ in that one string, so their goldens are re-blessed. Docs state the run-time rule in `reference/http.md`.
