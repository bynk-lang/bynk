---
level: minor
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: A suite case now drives an `on ANY(…)` pass-through route: it is addressed by the method sent (`proxy.GET("/blog/*path", "/posts/x")`, and new `HEAD` and `OPTIONS` addresses on an `ANY` route), a trailing `RequestHeaders` sets the inbound headers, and the result is the handler's `PassThrough`; an `Upstream` instance in a suite runs the real `forward` policy and hands what it built to `exchange`, its network step, which a case stubs with `Forwarded.of(status, headerLines, body)` and observes as an `UpstreamRequest` (`expect Pages.exchange called once with sent.headers.get("cookie") == None`); `exchange` and `Forwarded.of` are refused outside a suite (`bynk.test.upstream_exchange_outside_suite`, `bynk.test.forwarded_outside_suite`), a `system` case can't address an `ANY` route (`bynk.test.pass_through_system_address`), and `bynk.test.pass_through_address` is retired; `forward`'s answer already lacks `Set-Cookie`, `proxy-authenticate`, hop-by-hop headers and the upstream's HSTS; experimental
---

## ADR: pass-through-test-double
title: A suite drives a pass-through route through `Upstream`'s real policy, with only `exchange` doubled
summary: D20 of the pass-through track: the suite-only `exchange` seam beneath `forward`, `Forwarded.of`, `UpstreamRequest`, and the `ANY` route's suite addresses; relates to ADR `pass-through-router`

**Context.** Slice 3b (ADR `pass-through-router`) built the pass-through route and
`Upstream.forward`, and refused every suite case that addressed an `ANY` route
(`bynk.test.pass_through_address`), since a suite had no double for the upstream. The track
(`design/tracks/pass-through-http.md`, spine #3752) decides that double in D20: two seams,
`forward` for the handler's own logic and `exchange` beneath the forwarding policy, so the
§8 rows the policy holds get unit-tier cases that run the shipped code.

**Decision.** This records D20.

- **`exchange`.** `capability Upstream` declares `exchange(sent: UpstreamRequest) ->
  Effect[Result[Forwarded, UpstreamError]]`, the network step. Its only use is a suite's
  stub and observation:
  - The checker refuses a call outside a suite, in every unit kind: a context, and a
    commons, a dependency's included, through a capability parameter bounded by `Upstream`
    (`bynk.test.upstream_exchange_outside_suite`).
  - Every platform binding's `exchange` faults, so a call that reached it anyway still
    sends nothing.
  - Completion and signature help offer `exchange` only in a suite file, and its hover says
    it is suite-only.
- **`UpstreamRequest`.** `method`, `url`, `headers` (lower-case names) and `hasBody`: what
  the policy built, as a suite reads it.
- **The double.** A suite's `Upstream` instance is the runtime's `upstreamDouble`:
  - Its `forward` is the shipped policy, `upstreamExchange`, which `upstreamForward` also
    runs. That covers the path checks, the textual URL and its origin, the method and body,
    manual redirects and no inbound header.
  - Only the send differs: the double hands what the policy built to the instance's own
    `exchange`, which faults until a case stubs it.
  - A stub overlay calls `forward` with itself as `this`, so the stubbed or recorded
    `exchange` is the one reached.
  - Stubbing `forward` replaces the policy as well, for a case about the handler alone.
- **The answer.** `forward` strips the headers a pass-through answer never passes on before
  it returns: hop-by-hop ones, `proxy-authenticate`, `set-cookie` and the upstream's HSTS.
  So a suite's `Forwarded` is the answer the router passes through, and the router still
  strips them (D9).
- **`Forwarded.of(status, headerLines, body)`** builds an upstream's answer in a suite (a
  case, a suite helper or a stub value), and is refused anywhere else
  (`bynk.test.forwarded_outside_suite`), so a program can't fabricate one.
- **Addresses.** An `ANY` route is addressed by the method sent, one of the seven it
  forwards, then the pattern and its parameters:
  - `HEAD` and `OPTIONS` address only an `ANY` route.
  - `svc.ANY(…)`, which names no method, is refused.
  - An ordinary route with the same pattern and method keeps the address (D5).
  - The handler's `request` is built from the method and a trailing `RequestHeaders`, which
    is allowed below `system` and may hold an `Authorization`. It has no query or body.
  - The result is the handler's `Effect[PassThrough]`.
- **Tiers.** A `system` case can't address an `ANY` route
  (`bynk.test.pass_through_system_address`). The deployed Worker has no upstream, and the
  router's own answers are tested on workerd against a local upstream (§10.2).
- **`bynk.test.pass_through_address` is retired.** It never shipped in a release, so it has no
  tombstone, and its published page redirects to the testing reference.

Rejected:
- **A double that reimplements the policy.** The unit-tier SSRF rows would then test a copy,
  not the code that ships.
- **Stubbing `Fetch` underneath.** Pass-through doesn't go through `Fetch`, and a `Fetch`
  stub would bypass the policy (the track's own rejection).
- **Refusing `exchange` at run time only.** A program that called it would be accepted, and
  the policy would be skippable in principle.

**Consequences.**
- Every SSRF and header row of §8 that `forward` holds has a unit-tier case that runs the
  shipped policy (`positive/3752_pass_through_double`), beside the runtime's own tests.
- A unit-tier inbound request has no query or body yet, so `sent.url` never carries a query
  and `sent.hasBody` is false. Slice 4's header policy adds what a case needs.
- `HandlerAddress::Http` carries the position of an `ANY` handler's `request`, as a cron or
  queue address carries its parameter's.
