---
level: minor
changelog: "`headers.get(\"X\")?` in an `HttpResult` handler warns (`bynk.http.header_question_not_found`) with a `let … else` fix answering `400`; hover on `?` says `None → 404 NotFound`; `==`/`!=` against a bare `HttpResult`/`QueueResult` variant type-checks"
probes: diagnostic_coverage, span_keyed_maps, emit_diagnostics
---

## ADR: header-question-answers-404-lint
title: A header read under `?` is linted, because `?` on an Option means 404
summary: Warn on RequestHeaders.get(…)? in HttpResult handlers, fix to let … else; type bare variants under ==/!=

**Context.** `?` on an `Option` in an `HttpResult` handler answers `404
NotFound` (ADR 0177). For a missing request header that is the wrong status
(`400`), and it is the shortest spelling, so nothing steers the author to the
`let … else` form (#2010) that names the status. Separately, `expect r ==
BadRequest("msg")` failed to type-check because a bare `HttpResult` variant fell
back to a `()` payload when typed with no expected type.

**Decision.** Keep the `Option → 404` lift, document it as "not found", and add
a non-failing warning, `bynk.http.header_question_not_found`, on `get` of a
`RequestHeaders` value under `?` in an `HttpResult` handler. Headers only:
query parameters, bodies and other `Option`s are not flagged. When the `?` is
the whole value of an unannotated `let name = …`, a machine-applicable fix
rewrites it to `let Some(name) = headers.get("X") else { BadRequest("missing
X") }`. Hovering the `?` of an `Option` says `None → 404 NotFound`. Bare
`HttpResult` and `QueueResult` variants join `None`/`Ok`/`Err` in
`needs_expected_type`, so `==`/`!=` type them against the other operand; the
`()` fallback stays for a variant with nothing to infer from.

**Consequences.** No grammar, runtime or emitter-semantics change. A test file
comparing a `QueueResult` with a bare variant now imports `QueueResult`. The
lint may widen to other request-derived `Option`s on evidence. The
`?`-on-`Result[T, HttpResult[_]]` extension is dropped; it belongs with the
declared-embedding design of #543.
