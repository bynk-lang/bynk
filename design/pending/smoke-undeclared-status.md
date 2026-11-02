---
level: patch
probes: ts_writes, diagnostic_coverage
changelog: "`bynk test --smoke` warns (`bynk.test.smoke_undeclared_status`) when a route answers a non-5xx status its handler does not declare, on stderr and as a `warnings` array on the case in `json` and `ndjson`"
---

## ADR: smoke-undeclared-status-warning
title: A smoke case warns when a route answers a status it does not declare
summary: The smoke's `answers` case passes but carries a `warnings` entry, from a response set `bynk` computes and the test runtime compares, reported as an optional last-but-one `Case` field

**Context.** `bynk test --smoke` (#2658) asserts only that a route's answer is not
a 5xx, so a route that answers a `409` where nothing in its handler returns one
passes without comment. This was Decision E of #2658, deferred (#2685) for three
reasons: the response set is walked from the handler's body and is not
exhaustive, so a hard `expect` would fail on the very false positives it exists
to avoid; a warning had no channel from the runner to the report; and the
response set lives in `bynk-ide`'s wire contract, which the `bynkc` binary
deliberately does not carry.

**Decision.**

- **A warning, never a failure.** The `answers` case still passes. A non-5xx
  status outside the route's declared set raises
  `bynk.test.smoke_undeclared_status`, naming the route, the status received and
  the statuses declared. There is no strict flag: the set is a walk of the
  handler's body (a helper's result is invisible to it), so promoting the warning
  to a failure would reintroduce the false positives. A 5xx is the case's own
  failure and is never also a warning. The malformed-body and no-credential cases
  expect `400` and `401` by design and are not compared.
- **The set is `bynk`'s, handed to `bynkc` in a temporary file.** `bynk
  test --smoke` computes `route -> statuses` with the same `wire_contract` walk
  `bynk openapi` documents (boundary-implicit `400`, `401`, `413`, `415` included)
  writes a JSON object keyed `<context> <service>
  <METHOD> <path>`. `bynk-ide` stays out of `bynkc`, `bynk-driver` and
  `bynk-emit`.
- **The runtime compares.** A smoke build (a suite whose source is the
  synthesised `<stem>.smoke.bynk`) makes each typed route driver pass its key as
  a third argument to `responseToHttpResult`, which, under the smoke's strict
  mode, looks the status up in the table. No other build's emitted text changes.
- **The channel is the runner's stream.** Under `ndjson` the runtime writes a
  `{"type":"warning", …}` line before the case's own event, and the driver attaches
  the pending warnings to the next `case` event it reads; otherwise the runtime
  writes `warning[code]: message` to stderr. The driver also writes a
  `bynk test: warning[…]` line to stderr under `json` and `ndjson`, so the
  warning is on stderr in every format.
- **Schema.** `Case` gains an optional `warnings`: an array of `{ code, route,
  status, declared, message }`, appended after `origin` and omitted when empty, so
  every existing document is unchanged. A `case` event carries it before
  `durationMs`, which stays last. A `warning` line is internal to the runner's
  protocol and is not an event of the stream.

**Consequences.** A direct `bynkc test --smoke` (not through `bynk`) has no table
and compares nothing, so it does not warn. A route the table does not hold is not
compared. Two contexts' services of one name, method and path share a key, and
their declared sets are unioned, which can only under-warn. A route whose handler
returns a helper's `HttpResult` reports statuses from the helper as undeclared; the
warning is the intended place for that to surface, and a consumer that does not
want it ignores `warnings`.
