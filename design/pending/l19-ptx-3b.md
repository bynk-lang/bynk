---
level: minor
probes: emit_diagnostics, diagnostic_coverage, explained_codes, ts_writes
changelog: An `on ANY(…)` pass-through route now builds and forwards: `Upstream.forward` sends the method, the path it is given, the raw query and the streamed body to the declared origin, refusing a dot segment or a path that could leave it (`Err(InvalidPath)`), and a `Forward` passes the upstream's status, headers and body through with no `ETag`, `304`, `HEAD` body, `405`, `Allow` or CORS synthesised, stripping hop-by-hop headers, `Set-Cookie` and the upstream's HSTS; a method outside the seven, or an `Upgrade`, is `501`; no inbound header is forwarded yet; a suite case can't yet address a pass-through route (`bynk.test.pass_through_address`); experimental
---

## ADR: pass-through-router
title: A pass-through route forwards through `Upstream.forward` and suspends the router's own answers on the paths it owns
summary: D5 ownership at run time, the D9 suspensions, `501` for other methods and `Upgrade`, and `Upstream.forward` with the D13 path checks; relates to ADR 0159, 0162, 0163 and 0164

**Context.** The first half of slice 3 (ADR `pass-through-route-form`) gave a program the
route form, `on ANY(…)` returning `Effect[PassThrough]`, and its types, and refused every
build that reached one. The track (`design/tracks/pass-through-http.md`, spine #3752) says
what a pass-through answer keeps and suspends (D9), which requests the route owns (D5), and
how `Upstream.forward` reaches its origin (D13). This increment builds them. The forwarding
policy (D14, D16, the `Cache-Control` and `Vary` rules) is slice 4's, and the unit-tier
double (D20) is slice 3c's.

**Decision.** This records D5 (at run time), D7's `501`, D9 except slice 4's rows, D13's
exchange, D15's method and body, and D19.

- **Ownership (D5).** Each `ANY` route yields the paths it shares with an ordinary route (of
  any method) that ties or beats it: `route_order::pass_through_yields`, the same rule
  `bynk.http.unreachable_route` reads. On a yielded path the ordinary route answers, or its
  `405` with `Allow`. On every other path it matches, the `ANY` route answers every method.
  A CORS service's preflight never answers for a path an `ANY` route owns (D10).
- **Methods (D7, D15).** `GET`, `HEAD`, `POST`, `PUT`, `PATCH`, `DELETE` and `OPTIONS` reach
  the handler. Any other method, and an `Upgrade` request, is answered `501` before it runs.
- **The suspensions (D9).** A `Forward` is the upstream's status, headers and body,
  streamed. The router adds no weak `ETag` and no `304`, doesn't strip a `HEAD` body (`HEAD`
  was forwarded as `HEAD`), and has no `405`, `Allow` or CORS of its own on the path. It
  strips hop-by-hop headers and the names listed in `Connection`, `proxy-authenticate`,
  `set-cookie` (slice 4 adds `@responseCookies`) and the upstream's
  `strict-transport-security`. It stamps `nosniff` and the service's own HSTS, and fills
  `Content-Security-Policy` and `Referrer-Policy` on an HTML answer only where the upstream
  sent none. An upstream `101` is `502`. A `Respond` is an ordinary `HttpResult[()]`
  answer, without CORS, and to a `HEAD` it has no body. The router's own refusals keep their
  shape and `on rejected`.
- **The exchange (D13, D15, D19).** `Upstream.forward` is one function in the runtime,
  `upstreamForward`, which the compose hands to each platform binding's `UpstreamProvider`
  with the instance's origin and head timeout. So every platform forwards alike, and no
  binding imports a runtime shape (`emit_abi_shapes`).
  - It refuses a `path` that doesn't start with `/`, holds `?`, `#` or a control character,
    or has a `.` or `..` segment, encoded or not, with `\` read as `/`.
  - It also refuses a segment whose decoding holds `/` or `\` with a `.` or `..` component,
    or a control character. A segment is decoded up to three times, because a middlebox may
    decode again (`%252e%252e`), and one still encoded after the third is refused.
  - It builds the URL textually and compares its origin with the declared one before
    sending.
  - It sends the method and raw query, and the body for any method but `GET` and `HEAD`,
    streamed, with `redirect: "manual"`.
  - `@timeout` bounds the head: `Err(Timeout)`. A network failure is `Err(Unreachable)`.
  - A request is forwarded at most once.
  - It forwards no inbound header; slice 4's policy decides which pass.
- **Codecs.** A type holding an `InboundRequest` or a `Forwarded` (`PassThrough` among them)
  is not a boundary type and gets no codec. `InboundRequest` and `Forwarded` are emitted as
  the runtime's views under their brand, with an empty companion.
- **`bynk.http.pass_through_not_emitted` is retired.** Every target builds the route.
- **Suites.** A suite case that addresses an `ANY` route is
  `bynk.test.pass_through_address`, a temporary refusal: slice 3c adds the test double
  (`Upstream.exchange`, `Forwarded.of`, the `HEAD` and `OPTIONS` addresses) and retires it.

Rejected:
- **A route-level flag the runtime checks per response.** A suspension would then depend on
  the value at run time, not on the route.
- **The exchange in each platform binding.** Three copies of the SSRF checks, and new
  runtime imports in every binding.
- **Forwarding the inbound headers now.** Without D14's deny list that would send the
  proxy's own cookies and credentials to another origin.

**Consequences.**
- A pass-through route serves a site under a path at every target, with the SSRF checks of
  §8 refused before anything is sent. It isn't yet a useful proxy: the upstream gets no
  `If-None-Match`, `Accept` or `Cookie` until slice 4.
- ADR 0162's `HEAD` and `405`, ADR 0163's `ETag` and `304`, ADR 0159's CORS and ADR 0164's
  security headers each gain one exception, on the paths an `ANY` route owns, and the last
  fills rather than overwrites there.
- Until slice 3c, a pass-through route is tested against a real upstream, as the workerd
  case does.
